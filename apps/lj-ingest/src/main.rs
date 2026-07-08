//! Binaire `librejustice` (CLI ingest, clap). Port de `apps/ingest/.../cli.py`.

mod analyze;
mod chunking;
mod citations_maintenance;
mod config;
mod indexnow;
mod logging;
mod pipeline;
mod reverses;
mod sitemap;
mod summary;
mod tombstones;

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use lj_telemetry::{InitOpts, OtlpCreds, TelemetryGuard};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;

use crate::config::Settings;

/// Sous-dossier source opendata sous `cache_dir` (port de
/// `librejustice_core.sources.downloader.SOURCE_DIR`).
const OPENDATA_SOURCE_DIR: &str = "opendata_conseil_etat";
/// Sous-dossier source Judilibre sous `cache_dir` (port de
/// `librejustice_core.sources.judilibre.downloader.SOURCE_DIR`).
const JUDILIBRE_SOURCE_DIR: &str = "judilibre";
/// Juridictions Judilibre par défaut (port de
/// `judilibre.DEFAULT_JURISDICTIONS`).
const JUDILIBRE_DEFAULT_JURISDICTIONS: &[&str] = &["cc", "ca", "tj", "tcom"];
/// URL de prod de l'API PISTE Judilibre (port de `client.JudilibreClient`
/// `base_url` par défaut).
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
    ///
    /// Port de l'option `--log-level` du callback Typer `_main`.
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
enum Command {
    /// Sync incrémental des sources (opendata ZIP + Judilibre).
    Sync,
    /// Calcule les plages de dates Judilibre à synchroniser.
    JudilibreRanges,
    /// Analyse statistique d'un corpus local.
    Analyze,
    /// Applique les migrations Postgres.
    Migrate,
    /// Ingère les archives locales en base (parse → chunk → upsert).
    Ingest {
        #[arg(long)]
        with_embeddings: bool,
        /// Mode de triage. `missing-hash` (défaut) : skip si hash identique +
        /// fast-skip manifeste. `all` : relit tout, ignore hash et manifeste,
        /// force un UPDATE complet (ré-chunk/ré-embed après un changement du
        /// pipeline pur).
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
    /// Re-fetch ciblé de décisions Judilibre par id (`/decision`) puis ingest.
    /// Répare quelques décisions désynchronisées (ex. résurrection, ADR 0087)
    /// sans reculer le watermark global.
    Refetch {
        /// Ids Judilibre à re-fetch (ex. `6a28fc8ecdc6046d47cb0167`).
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long)]
        with_embeddings: bool,
    },
    /// Ré-extrait les champs structurés depuis les payloads stockés.
    ReextractFields {
        #[arg(long)]
        overwrite: bool,
        /// Passe intégrale (ADR 0145) : TOUT le fonds `extract_version < 1000`,
        /// même déjà à la version courante — c'est le relink hebdomadaire (un
        /// texte nouveau au catalogue attire ses citations anciennes ; le skip
        /// des sets de citations inchangés borne les écritures au delta).
        #[arg(long, conflicts_with_all = ["juridiction_type", "citing_ref_uid"])]
        full: bool,
        /// Cible des `juridiction_type` (CSV, ex. `CNDA,CEDH,CJUE,CONSTIT,TC`) à
        /// re-extraire **quelle que soit la `extract_version`** (sans bump global).
        /// Pour activer un comportement d'extraction nouveau sur un sous-ensemble
        /// (citations famille générique, ADR 0102 §B). Vide = comportement par
        /// défaut (toutes les versions divergentes).
        #[arg(long, value_delimiter = ',')]
        juridiction_type: Vec<String>,
        /// Champs à re-extraire (CSV, ex. `legal_references`). Vide = tous les
        /// champs ré-extractibles. Restreindre à `legal_references` ne ré-écrit
        /// que les citations (via `replace_citations`), sans toucher les colonnes.
        #[arg(long, value_delimiter = ',')]
        field: Vec<String>,
        /// Cible les décisions **citant** ce texte (`ref_text_uid`, ADR 0145 M4),
        /// quelle que soit la `extract_version`. Pour rejouer extract+link sur le
        /// seul gisement d'un instrument, sans bump global.
        #[arg(long)]
        citing_ref_uid: Option<String>,
        /// Workers concurrents = connexions DB simultanées (chacun fait
        /// fetch→extract→write en parallèle). C'est le levier de pression sur la
        /// base : `--workers 2` reste doux sur une prod qui sert du trafic ;
        /// vide = défaut agressif (cœurs−4, borné [2,8]) pour un drain rapide.
        #[arg(long)]
        workers: Option<usize>,
    },
    /// Re-embed ciblé des décisions orphelines (chunks sans embedding, #39).
    /// Reconstruit depuis `(full_text, source_fields)` (ADR 0085) et ne remplit
    /// que les embeddings manquants — vLLM strict (jamais Cloudflare, coût).
    EmbedMissing {
        /// Borne le nombre de décisions traitées (défaut : toutes).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Purge les liens de réversion (reverses).
    PurgeReverses,
    /// Backfill hors-migration des provenances existantes vers `decision_sources`
    /// (ADR 0080), batché par keyset id et repris (idempotent). Sorti de la
    /// migration 0056 pour ne pas tenir une transaction longue (base low-IOPS).
    BackfillDecisionSources,
    /// Génère les résumés LLM manquants.
    GenerateSummaries {
        #[arg(long)]
        limit: Option<usize>,
        /// Appels Mistral concurrents (sémaphore). Les clés API tournent en
        /// round-robin par requête → la concurrence soutenable est proportionnelle
        /// au nombre de clés. Défaut (absent) : dérivé du pool (~5 RPS/clé × 1,5 s
        /// de latence ≈ 7 requêtes en vol/clé), pour saturer sans franchir le quota.
        #[arg(long)]
        concurrency: Option<usize>,
    },
    /// Recompute des arrays légaux dénormalisés depuis `legal_citation`
    /// (réparation / recompute, migration 0098). Idempotent, lots autocommit ;
    /// en régime normal l'écrivain de citations tient les arrays à jour.
    ResyncLegalArrays,
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
    /// Ingère un stock ou un incrément LEGI (référentiel versionné, ADR 0092)
    /// depuis un `tar.gz` bulk DILA local (`Freemium_legi_global_*.tar.gz` ou
    /// incrément `LEGI_*.tar.gz`, téléchargé hors-bande).
    Legi {
        /// Chemin local de l'archive `tar.gz` LEGI.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Sync incrémental LEGI (ADR 0092/0093) : télécharge les incréments
    /// `LEGI_*.tar.gz` postérieurs au watermark puis les ingère (le stock global
    /// se bootstrape hors-bande via `legi <path>`). Idempotent.
    SyncLegi,
    /// Ingère un stock ou un incrément KALI (conventions collectives nationales,
    /// bulk DILA, ADR 0120) depuis un `tar.gz` local (`Freemium_kali_global_*.tar.gz`
    /// ou incrément `KALI_*.tar.gz`). KALICONT → `legal_text`, KALIARTI → `legal_article`
    /// (ancrés sur la convention). Idempotent.
    Kali {
        /// Chemin local de l'archive `tar.gz` KALI.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Sync incrémental KALI (ADR 0120/0093) : cold start télécharge le stock global
    /// puis les incréments `KALI_*.tar.gz` postérieurs au watermark, chacun ingéré.
    /// Idempotent.
    SyncKali,
    /// Ingère un stock ou un incrément JORF (Journal officiel, bulk DILA, ADR 0109)
    /// depuis un `tar.gz` local (`Freemium_jorf_global_*.tar.gz` ou incrément
    /// `JORF_*.tar.gz`). Deux passes : textes (`referential_texts`, tag
    /// `treaty`/`jorf`) puis articles numérotés (`referential_articles`). Idempotent.
    Jorf {
        /// Chemin local de l'archive `tar.gz` JORF.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Charge un corpus de loi curé générique (datasets `<state_dir>/sources/
    /// legal-corpus/*.json`, règle #17) → `legal_text` + `legal_article`. Source-
    /// agnostique : le JSON porte les métadonnées du texte + des `articles[]` **déjà
    /// segmentés par la curation Python** (ADR 0118), chacun mono-version (`texte`)
    /// ou multi-versions (`versions[]`, avenants des traités). Le loader est un pur
    /// inséreur (seule transformation : `num_key = normalize_article`). Dataset
    /// autoritaire (purge avant rechargement). Idempotent. ADR 0108/0109/0118.
    /// Assigne les slugs manquants de `legal_text` (ADR 0162) : backfill puis
    /// no-op (la passe tourne aussi en fin de chaque ingest référentiel).
    AssignSlugs,
    LoadLegalCorpus {
        /// Restreint le chargement aux datasets dont le nom contient cette sous-chaîne
        /// (chargement chirurgical, ex. `--only eu-rproc`). Défaut : tous les datasets.
        #[arg(long)]
        only: Option<String>,
    },
    /// Génère les datasets catalogue EUR-Lex (règlements/directives UE cités mais
    /// absents) sous `<state_dir>/sources/legal-corpus/`, via SPARQL Cellar, pilotés par
    /// les slashnums réellement cités (spans non liés de `legal_citation`, ADR
    /// 0138/0145). Entrées catalogue-seul (`articles: []`) ; auto-validées (le titre
    /// FR doit porter le slashnum cité). À enchaîner avec `load-legal-corpus` ; les
    /// citations s'attachent à la passe intégrale suivante. `--limit` borne aux N
    /// slashnums les plus cités (rodage). Idempotent (skip datasets présents).
    IngestEuCatalog {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Ingestion catalogue des règlements de procédure des juridictions UE (Cour de
    /// justice, Tribunal, ex-TFP) — corpus gap ADR 0137 (≈25 K mentions CJUE). Écrit
    /// des datasets catalogue curés (variantes = alias TSV du linker) ; enchaîner
    /// avec `load-legal-corpus`. `--dry-run` liste sans écrire. Idempotent.
    IngestEuRproc {
        #[arg(long)]
        dry_run: bool,
    },
    /// Rafraîchit la fraîcheur « as-of » des sources *live* autoritaires
    /// (legifrance/kali/jorf) à la date du jour dans `ingest_freshness` (ADR 0129) :
    /// re-confirme que la copie reflète le droit en vigueur. À lancer après chaque
    /// ingest LEGI/KALI/JORF (cron). Idempotent (une ligne par source).
    StampFreshness,
    /// Retrofit ADR 0131 : reclasse les `source` catégorie/méthode hérités (`treaty`,
    /// `eu-law`, `official-fr`, `traduction-automatique`) en libellés de diffuseur réels,
    /// dérivés de `source_url`. Idempotent ; lignes sans URL hors bulk JORF laissées au
    /// reload de curation (loguées). One-shot après déploiement.
    RelabelSources,
    /// Canonicalise les variantes d'hôte d'un diffuseur (`jafbase.fr` → `jafbase`) vers
    /// son libellé unique (ADR 0131). Idempotent. One-shot post-déploiement.
    CanonicalizeSourceLabels,
    /// Ingère un fond bulk DILA (`tar.gz` locaux sous `<cache>/dila/<fond>/tarballs/`,
    /// ADR 0093) : `jade` (admin CE/CAA/TC), `constit` (Conseil constitutionnel).
    /// Idempotent ; dédup ECLI-first (ADR 0080). INCA retiré (superset Judilibre, ADR 0099).
    IngestDila {
        /// Fond DILA à ingérer.
        #[arg(value_enum)]
        fond: pipeline::Fond,
    },
    /// Sync incrémental d'un fond DILA (télécharge les incréments postérieurs au
    /// watermark, ADR 0093). N'ingère rien — `ingest-dila` consomme les archives.
    SyncDila {
        /// Fond DILA à synchroniser.
        #[arg(value_enum)]
        fond: pipeline::Fond,
    },
    /// Backfill de la colonne `decisions.ecli` depuis `source_fields->>'ecli'`
    /// (ADR 0093) : rend la dédup ECLI-first (ADR 0080) active sur les lignes
    /// historiques (Judilibre porte l'ECLI dans son payload, colonne NULL).
    /// Keyset batché, idempotent.
    BackfillEcli,
    /// Récupération rétroactive de la dédup inter-sources (ADR 0098 §7 / 0100).
    /// Enchaîne les passes batchées idempotentes : (1) portage des provenances vers
    /// `decision_sources` + `source_fields`, (2) calcul de `decisions.canonical_ref`
    /// (citation légale, ADR 0100). Les passes 3 (clustering) et 4 (fusion des
    /// doublons) viennent ensuite. Reprenable, hors live path.
    DedupBackfill,
    /// Re-dérive `decisions.canonical_ref` (ADR 0100). Sans `--force` : ne traite
    /// que les `NULL` (peuplement). Avec `--force` : **re-dérive toutes** les
    /// décisions (`full_text` présent) pour migrer les clés 3-champs historiques
    /// `{nom}|{rg}|{date}` en 4-champs `{type}|{location}|{rg}|{date}` (fix
    /// cross-court 2026-06-15). Pur calcul, aucun re-embed ; une clé `None` laisse
    /// l'existante (jamais d'écrasement vers `NULL`).
    BackfillCanonicalRef {
        /// Re-dérive toutes les décisions (pas seulement `canonical_ref IS NULL`).
        #[arg(long)]
        force: bool,
    },
    /// Fusion rétroactive des **faux splits cross-source** (ADR 0098/0100/0106) :
    /// décisions distinctes au même `canonical_ref` avec sources **disjointes** =
    /// même décision multi-sources (p.ex. 33 CAA JADE↔opendata réalignées par la
    /// clé RG). Garde l'autorité (rang max), fusionne les autres dedans. Jamais de
    /// fusion intra-source (ADR 0104). Aucun re-embed. Idempotent.
    MergeCrossSourceDuplicates,
    /// Re-ingest **ciblé** des décisions opendata dont l'autorité a basculé sur
    /// opendata (rang 55 généré, ADR 0109) mais dont le `full_text` reste figé sur
    /// la provenance rang 50 (jade/constit/cedh/cjue/cnda) qui gagnait quand
    /// opendata valait 40 — ~61,7k CAA/CE. Re-parse leur payload opendata (seuls les
    /// ZIPs portant une cible sont ouverts) en mode All : re-chunk + re-embed (vLLM
    /// strict, jamais Cloudflare) + réécriture du `full_text` opendata. One-shot,
    /// hors cron. Idempotent (rejoué, ne re-flippe que ce qui n'est pas déjà à jour).
    ReingestStaleOpendata,
    /// Analyse **read-only** des faux merges judilibre (#29 / ADR 0100) : recalcule
    /// `canonical_ref` par provenance sur les décisions multi-provenances
    /// tout-judilibre et compte légitimes / ambiguës / faux merges (clés
    /// divergentes). N'écrit rien — mesure l'ampleur avant le re-split.
    AnalyzeFalseMerges,
    /// Réparation des faux merges judilibre (#29 / ADR 0100 §5). **`--dry-run` par
    /// défaut** (read-only) : recalcule `canonical_ref` par provenance, planifie le
    /// scinde (groupe gardé = celui de l'autorité ; groupes divergents à
    /// reconstituer) et émet le PLAN sans **aucune** écriture. `--execute` (write,
    /// gaté JADE + 0065 + GPU libre) : crée les décisions scindées, re-fetch les
    /// `source_uid` divergents + le groupe gardé + ré-ingère (re-chunk + re-embed
    /// ciblé vLLM local, jamais Cloudflare). Transaction par cluster, reprise
    /// idempotente. `--limit` borne le 1ᵉʳ rollout (s'arrête après N clusters
    /// réparés) pour auditer avant la passe complète.
    ResplitFalseMerges {
        /// Applique réellement le scinde (write). Sans ce flag : dry-run read-only.
        #[arg(long)]
        execute: bool,
        /// Taille de l'échantillon de plan imprimé.
        #[arg(long, default_value_t = 25)]
        audit_sample: usize,
        /// Borne le nombre de clusters **réparés** (write) avant arrêt — rollout
        /// prudent. Sans valeur : pas de borne. Ignoré en dry-run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Défait les faux merges **intra-source** judilibre (ADR 0104) : détache
    /// chaque provenance perdante (≥2 judilibre/décision) vers sa propre décision
    /// et la re-matérialise **depuis le cache local** (re-chunk + re-embed vLLM).
    UnmergeSameSource {
        /// Applique réellement (write + GPU). Sans ce flag : dry-run read-only.
        #[arg(long)]
        execute: bool,
        /// Borne le nombre de splits écrits avant arrêt (rollout prudent). Sans
        /// valeur : pas de borne. Ignoré en dry-run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Défait les faux merges **intra-source** `dila-jade` (ADR 0104, #47) :
    /// analogue de `unmerge-same-source` pour le fond DILA JADE. Re-matérialise
    /// chaque perdante depuis les **tarballs locaux** (streaming, re-embed vLLM).
    UnmergeSameSourceDila {
        /// Applique réellement (write + GPU). Sans ce flag : dry-run read-only.
        #[arg(long)]
        execute: bool,
        /// Borne le nombre de splits écrits avant arrêt (rollout prudent). Sans
        /// valeur : pas de borne. Ignoré en dry-run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Bootstrap CEDH (HUDOC) : balaye toutes les années, fetch les arrêts FR
    /// (HFJUD) et les ingère (ADR 0094). Idempotent par `content_checksum`.
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
    /// Bootstrap CJUE (EUR-Lex) : balaye toutes les années, fetch le texte FR des
    /// arrêts/ordonnances et les ingère (ADR 0094). Idempotent.
    IngestCjue,
    /// Sync incrémental CEDH : ne balaye que l'année courante (ADR 0094).
    SyncCedh,
    /// Sync incrémental CJUE : ne balaye que l'année courante (ADR 0094).
    SyncCjue,
    /// Bootstrap CNDA (Cour nationale du droit d'asile, ADR 0096) : crawl la liste
    /// jurisprudentielle paginée de `cnda.fr`, fusionne fiche HTML + PDF lié par
    /// numéro et les ingère (`source_uid = cnda/<numero>`, ECLI fabriqué).
    /// Idempotent par `content_checksum`. `--mode all` force la ré-extraction
    /// complète (re-OCR + ré-chunk/ré-embed) de toutes les décisions, même à
    /// PDF inchangé — utilisé après un changement d'extraction (bascule OCR).
    /// `--only <numéro>` (répétable) : ne ré-ingère que ces décisions précises —
    /// crawl la liste mais ne traite/persiste que les numéros visés (re-OCR forcé,
    /// ne touche pas le watermark). Pour récupérer un scanné jamais persisté ou
    /// rafraîchir une décision sans re-balayer tout le corpus.
    IngestCnda {
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
        #[arg(long)]
        only: Vec<String>,
    },
    /// Sync incrémental CNDA : reprend le crawl à la page suivant le watermark
    /// (ADR 0096).
    SyncCnda,
    /// Maintenance DB (sous-commandes).
    #[command(subcommand)]
    Db(DbCommand),
}

#[derive(Subcommand)]
enum DbCommand {
    /// ANALYZE des tables.
    Analyze,
    /// Reconstruit l'index de recherche.
    ReindexSearch,
    /// VACUUM FULL sur les chunks.
    VacuumFullChunks,
    /// GC des tokens/clients MCP expirés.
    GcMcp,
    /// Préchauffe le cache (prewarm).
    Prewarm,
}

impl Command {
    /// Libellé stable de phase pour les breadcrumbs / le watchdog (clé de requête
    /// Loki : `phase="ingest"`). Statique, indépendant des args.
    fn label(&self) -> &'static str {
        match self {
            Command::Sync => "sync",
            Command::JudilibreRanges => "judilibre-ranges",
            Command::Analyze => "analyze",
            Command::Migrate => "migrate",
            Command::Ingest { .. } => "ingest",
            Command::Refetch { .. } => "refetch",
            Command::ReextractFields { .. } => "reextract-fields",
            Command::EmbedMissing { .. } => "embed-missing",
            Command::PurgeReverses => "purge-reverses",
            Command::BackfillDecisionSources => "backfill-decision-sources",
            Command::GenerateSummaries { .. } => "generate-summaries",
            Command::ResyncLegalArrays => "resync-legal-arrays",
            Command::Sitemap { .. } => "sitemap",
            Command::Indexnow { .. } => "indexnow",
            Command::Legi { .. } => "legi",
            Command::SyncLegi => "sync-legi",
            Command::Kali { .. } => "kali",
            Command::SyncKali => "sync-kali",
            Command::Jorf { .. } => "jorf",
            Command::AssignSlugs => "assign-slugs",
            Command::LoadLegalCorpus { .. } => "load-legal-corpus",
            Command::IngestEuCatalog { .. } => "ingest-eu-catalog",
            Command::IngestEuRproc { .. } => "ingest-eu-rproc",
            Command::StampFreshness => "stamp-freshness",
            Command::RelabelSources => "relabel-sources",
            Command::CanonicalizeSourceLabels => "canonicalize-source-labels",
            Command::IngestDila { .. } => "ingest-dila",
            Command::SyncDila { .. } => "sync-dila",
            Command::BackfillEcli => "backfill-ecli",
            Command::DedupBackfill => "dedup-backfill",
            Command::BackfillCanonicalRef { .. } => "backfill-canonical-ref",
            Command::MergeCrossSourceDuplicates => "merge-cross-source-duplicates",
            Command::ReingestStaleOpendata => "reingest-stale-opendata",
            Command::AnalyzeFalseMerges => "analyze-false-merges",
            Command::ResplitFalseMerges { .. } => "resplit-false-merges",
            Command::UnmergeSameSource { .. } => "unmerge-same-source",
            Command::UnmergeSameSourceDila { .. } => "unmerge-same-source-dila",
            Command::IngestCedh { .. } => "ingest-cedh",
            Command::CacheCedh { .. } => "cache-cedh",
            Command::IngestCjue => "ingest-cjue",
            Command::SyncCedh => "sync-cedh",
            Command::SyncCjue => "sync-cjue",
            Command::IngestCnda { .. } => "ingest-cnda",
            Command::SyncCnda => "sync-cnda",
            Command::Db(DbCommand::Analyze) => "db:analyze",
            Command::Db(DbCommand::ReindexSearch) => "db:reindex-search",
            Command::Db(DbCommand::VacuumFullChunks) => "db:vacuum-full-chunks",
            Command::Db(DbCommand::GcMcp) => "db:gc-mcp",
            Command::Db(DbCommand::Prewarm) => "db:prewarm",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings::from_env()?;

    // Callback global (port de `_main`) : installe le subscriber + l'export OTLP
    // avant toute sous-commande. Le guard est tenu vivant pour tout le process :
    // son `Drop` flush les batch processors (cron court-vivant → sans ça les
    // derniers spans/logs ne partiraient pas vers Grafana Cloud).
    let _telemetry_guard = init_telemetry(&cli.log_level, &settings)?;
    install_panic_hook();

    run_with_watchdog(cli.command, settings.watchdog_secs).await
}

/// Exécute la commande sous un watchdog + breadcrumbs de cycle de vie.
///
/// - **Watchdog** : au-delà de `watchdog_secs`, on logge une erreur et on
///   abandonne avec un `Err` (code non nul) au lieu de rester pendu. Le `&&` du
///   cron casse alors proprement (le 2026-06-11, faute de borne, la chaîne est
///   restée pendue ~7 h après l'ingest jusqu'au reboot). `0` = désactivé.
/// - **Breadcrumbs** : `info!` début + `info!`/`error!` fin, shippés vers Loki
///   (bridge ingest à INFO). Un « début » sans « fin » localise un hang.
///
/// Le timer du watchdog vit sur le runtime multi-thread : même si la commande
/// bloque un worker, le timeout se déclenche, `main` rend `Err`, le guard flush
/// les logs et le process sort (les threads orphelins meurent à l'exit).
async fn run_with_watchdog(command: Command, watchdog_secs: u64) -> Result<()> {
    let phase = command.label();
    let started = Instant::now();
    tracing::info!(phase, "commande lj-ingest : début");

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
            tracing::info!(phase, elapsed_s, "commande lj-ingest : succès");
            Ok(())
        }
        Ok(None) => {
            tracing::error!(
                phase,
                elapsed_s,
                watchdog_secs,
                "commande lj-ingest : watchdog — dépassement de délai, abandon"
            );
            Err(anyhow!(
                "watchdog: la commande `{phase}` a dépassé {watchdog_secs} s"
            ))
        }
        Err(e) => {
            tracing::error!(phase, elapsed_s, error = %e, "commande lj-ingest : échec");
            Err(e)
        }
    }
}

/// Hook de panique : logge la panique en `error!` (donc shippée vers Loki via le
/// bridge) avant de déléguer au hook par défaut (backtrace stderr). Le binaire
/// `lj-ingest` déroule (`panic=unwind`, seul le wasm est `abort`), donc le `Drop`
/// du guard flush la pile de logs — y compris cet `error!` — pendant l'unwind.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "panic lj-ingest");
        default(info);
    }));
}

/// Construit les `InitOpts` (niveau parité Python + format + creds OTLP via
/// `Settings`, regle #5) et installe la telemetrie partagee. `service.name =
/// librejustice-ingest`.
fn init_telemetry(log_level: &str, settings: &Settings) -> Result<TelemetryGuard> {
    let level = logging::resolve_level(Some(log_level));
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .parse_lossy("");

    // Creds OTLP lus via Settings (jamais d'env dispersé). Les trois presents →
    // export actif ; sinon subscriber fmt seul (dev local).
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
        // Ingest (cron, bas volume) : on ship INFO+ pour les breadcrumbs de phase.
        otlp_log_level: LevelFilter::INFO,
        otlp,
    })
}

/// Dispatch des sous-commandes (port du corps de chaque `@app.command` de
/// `cli.py`). Chaque branche construit `Settings`, le pool DB et les clients
/// nécessaires comme la commande Typer correspondante.
async fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Sync => cmd_sync().await,
        Command::JudilibreRanges => cmd_judilibre_ranges().await,
        Command::Analyze => cmd_analyze(),
        Command::Migrate => cmd_migrate().await,
        Command::Ingest {
            with_embeddings,
            mode,
        } => cmd_ingest(with_embeddings, mode).await,
        Command::Refetch {
            ids,
            with_embeddings,
        } => cmd_refetch(ids, with_embeddings).await,
        Command::ReextractFields {
            overwrite,
            full,
            juridiction_type,
            field,
            citing_ref_uid,
            workers,
        } => {
            cmd_reextract_fields(
                overwrite,
                full,
                juridiction_type,
                field,
                citing_ref_uid,
                workers,
            )
            .await
        }
        Command::EmbedMissing { limit } => pipeline::embed_missing(limit).await,
        Command::PurgeReverses => cmd_purge_reverses().await,
        Command::BackfillDecisionSources => pipeline::backfill_decision_sources().await,
        Command::GenerateSummaries { limit, concurrency } => {
            summary_pipeline_generate(limit, concurrency).await
        }
        Command::ResyncLegalArrays => citations_maintenance::resync_arrays().await,
        Command::Sitemap { dry_run } => cmd_sitemap(dry_run).await,
        Command::Indexnow {
            since_hours,
            max_urls,
        } => cmd_indexnow(since_hours, max_urls).await,
        Command::Legi { path } => pipeline::ingest_legi(&path).await,
        Command::SyncLegi => pipeline::sync_legi().await,
        Command::Kali { path } => pipeline::ingest_kali(&path).await,
        Command::SyncKali => pipeline::sync_kali().await,
        Command::Jorf { path } => pipeline::ingest_jorf(&path).await,
        Command::AssignSlugs => pipeline::assign_slugs().await,
        Command::LoadLegalCorpus { only } => pipeline::load_legal_corpus(only.as_deref()).await,
        Command::IngestEuCatalog { limit } => pipeline::ingest_eu_catalog(limit).await,
        Command::IngestEuRproc { dry_run } => pipeline::ingest_eu_rproc(dry_run).await,
        Command::StampFreshness => pipeline::stamp_freshness().await,
        Command::RelabelSources => pipeline::relabel_sources().await,
        Command::CanonicalizeSourceLabels => pipeline::canonicalize_source_labels().await,
        Command::IngestDila { fond } => pipeline::ingest_dila(fond).await,
        Command::SyncDila { fond } => pipeline::sync_dila(fond).await,
        Command::BackfillEcli => pipeline::backfill_ecli().await,
        Command::DedupBackfill => pipeline::dedup_backfill().await,
        Command::BackfillCanonicalRef { force } => pipeline::backfill_canonical_ref(force).await,
        Command::MergeCrossSourceDuplicates => pipeline::merge_cross_source_duplicates().await,
        Command::ReingestStaleOpendata => cmd_reingest_stale_opendata().await,
        Command::AnalyzeFalseMerges => pipeline::analyze_false_merges().await,
        Command::ResplitFalseMerges {
            execute,
            audit_sample,
            limit,
        } => cmd_resplit_false_merges(execute, audit_sample, limit).await,
        Command::UnmergeSameSourceDila { execute, limit } => {
            pipeline::unmerge_same_source_dila(execute, limit).await
        }
        Command::UnmergeSameSource { execute, limit } => {
            pipeline::unmerge_same_source(execute, limit).await
        }
        Command::IngestCedh { from_year } => pipeline::ingest_cedh(from_year).await,
        Command::CacheCedh { from_year } => pipeline::cache_cedh(from_year).await,
        Command::IngestCjue => pipeline::ingest_cjue().await,
        Command::SyncCedh => pipeline::sync_cedh().await,
        Command::SyncCjue => pipeline::sync_cjue().await,
        Command::IngestCnda { mode, only } => pipeline::ingest_cnda(mode, only).await,
        Command::SyncCnda => pipeline::sync_cnda().await,
        Command::Db(db_command) => cmd_db(db_command).await,
    }
}

/// Pool deadpool sur la DB cible + migrations idempotentes appliquées (factorise
/// le `db_connect(url); apply_migrations(conn)` répété dans `cli.py`).
async fn pool_with_migrations(settings: &Settings) -> Result<deadpool_postgres::Pool> {
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Pool deadpool sans migrations (pour les commandes de maintenance DB qui ne
/// veulent pas toucher au schéma).
async fn pool_only(settings: &Settings) -> Result<deadpool_postgres::Pool> {
    lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// sync (port de `cmd_sync`)
// ─────────────────────────────────────────────────────────────────────────────

/// Synchronise le cache local des deux sources (opendata + Judilibre), comme
/// `cmd_sync` sans argument (toutes les sources, options par défaut).
///
/// Le clap `Sync` ne porte pas les options Typer (`--juridictions`, `--from`,
/// `--max-workers`, …) : on appelle les syncers `lj-sources` avec leurs valeurs
/// par défaut, identiques aux défauts Typer (`earliest=DEFAULT_EARLIEST`,
/// `latest=None`, `force=False`).
async fn cmd_sync() -> Result<()> {
    let settings = Settings::from_env()?;
    let cache_dir = settings.cache_dir();

    // === sync opendata_conseil_etat === (port de `_sync_opendata_conseil_etat`).
    // Non-fatal : une panne transitoire opendata ne doit pas abandonner le reste
    // de la chaîne nocturne (Judilibre = décisions du jour). On log et on continue.
    tracing::info!("=== sync opendata_conseil_etat ===");
    if let Err(e) = lj_sources::downloader::sync_opendata(&cache_dir, false) {
        tracing::error!(error = %e, "sync opendata échoué — on poursuit avec Judilibre");
    }

    // === sync judilibre === (port de `_sync_judilibre`).
    tracing::info!("=== sync judilibre ===");
    let client = judilibre_client(&settings)?;
    // `cmd_sync` n'expose pas `--from` côté clap → `date_start=None` (chaîne
    // vide = défaut par juridiction MONTHLY_START, cf. `sync_judilibre`).
    lj_sources::downloader::sync_judilibre(&client, &cache_dir, "")
        .await
        .map_err(|e| anyhow!("sync judilibre: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// judilibre-ranges (port de `cmd_judilibre_ranges`)
// ─────────────────────────────────────────────────────────────────────────────

/// Une plage bornée par bisection : `(date_start, date_end, total)`.
type JudilibreRange = (String, String, i64);

/// Bisection one-shot des plages de bootstrap Judilibre (port de
/// `cmd_judilibre_ranges`, valeurs par défaut : toutes les juridictions,
/// `max_chunk=50_000`, `date_from="1900-01-01"`).
async fn cmd_judilibre_ranges() -> Result<()> {
    use chrono::{Duration, NaiveDate, Utc};

    let settings = Settings::from_env()?;

    let jurs: Vec<&str> = JUDILIBRE_DEFAULT_JURISDICTIONS.to_vec();
    let max_chunk: i64 = 50_000;
    let date_from = "1900-01-01";
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();

    let client = judilibre_client(&settings)?;

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

    // Bisection dichotomique itérative (pile explicite — évite la récursion
    // dans une `async fn`, qui exigerait un `Box<dyn Future>`). L'ordre de
    // sortie reste celui de la récursion gauche-d'abord du Python.
    async fn bisect(
        client: &lj_sources::judilibre::JudilibreClient,
        jur: &str,
        start: &str,
        end: &str,
        max_chunk: i64,
    ) -> Result<Vec<JudilibreRange>> {
        use chrono::Duration;
        let mut ranges: Vec<JudilibreRange> = Vec::new();
        // Pile LIFO : on empile (d_end, d_start) à l'envers pour dépiler dans
        // l'ordre gauche-d'abord.
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

    // Sortie copy-pastable (port de la sortie `print(...)` finale).
    println!("\n# BOOTSTRAP_RANGES — coller dans downloader.py");
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
// analyze (port de `cmd_analyze`)
// ─────────────────────────────────────────────────────────────────────────────

/// Passe d'analyse du corpus local (port de `cmd_analyze`, défauts :
/// `extra_dir=[]`, `sample=20`, `seed=0`, `out=None` → stdout).
fn cmd_analyze() -> Result<()> {
    let settings = Settings::from_env()?;
    let report = analyze::run(&settings.cache_dir().join(OPENDATA_SOURCE_DIR), &[], 20, 0)?;
    // `json.dumps(report, indent=2, ensure_ascii=False)` + newline final.
    let payload = serde_json::to_string_pretty(&report)?;
    println!("{payload}");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// migrate (port de `cmd_migrate`)
// ─────────────────────────────────────────────────────────────────────────────

/// Applique les migrations SQL idempotemment (port de `cmd_migrate`).
async fn cmd_migrate() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = pool_only(&settings).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let applied = lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    if applied.is_empty() {
        println!("Base déjà à jour.");
    } else {
        println!("Migrations appliquées : {applied:?}");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ingest (port de `cmd_ingest`)
// ─────────────────────────────────────────────────────────────────────────────

/// Ingère les deux sources connues (port de `cmd_ingest` avec `--sources all`).
///
/// On itère les sources dans l'ordre du router Python (`opendata_conseil_etat`
/// puis `judilibre`), chacune sur son sous-dossier de cache. `pipeline::ingest_*`
/// gère pool, migrations et embedder en interne. `mode` propage la trappe ALL
/// (re-traitement total) jusqu'au triage.
async fn cmd_ingest(with_embeddings: bool, mode: pipeline::IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let cache_dir = settings.cache_dir();
    pipeline::ingest_opendata(&cache_dir.join(OPENDATA_SOURCE_DIR), with_embeddings, mode).await?;
    pipeline::ingest_judilibre(&cache_dir.join(JUDILIBRE_SOURCE_DIR), with_embeddings, mode)
        .await?;
    Ok(())
}

/// Re-ingest ciblé des décisions opendata à `full_text` figé sur une provenance
/// rang 50 (ADR 0109). Scanne le dossier opendata, mais ne ré-embed que les
/// ~61,7k cibles (CAA/CE) ; vLLM strict.
async fn cmd_reingest_stale_opendata() -> Result<()> {
    let settings = Settings::from_env()?;
    let cache_dir = settings.cache_dir();
    pipeline::reingest_stale_opendata(&cache_dir.join(OPENDATA_SOURCE_DIR)).await
}

// ─────────────────────────────────────────────────────────────────────────────
// reextract-fields (port de `cmd_reextract_fields`)
// ─────────────────────────────────────────────────────────────────────────────

/// Ré-extrait les champs structurés depuis les payloads stockés (port de
/// `cmd_reextract_fields`). `--field` (vide → tous les champs ré-extractibles) ;
/// `--juridiction-type` (vide → keyset par version) cible un sous-ensemble quelle
/// que soit la version (ADR 0102 §B, citations famille générique) ;
/// `--citing-ref-uid` cible les décisions citant un texte donné (ADR 0145 M4) ;
/// `--full` = passe intégrale/relink hebdo (tout le fonds < 1000).
async fn cmd_reextract_fields(
    overwrite: bool,
    full: bool,
    juridiction_type: Vec<String>,
    field: Vec<String>,
    citing_ref_uid: Option<String>,
    workers: Option<usize>,
) -> Result<()> {
    let jts = (!juridiction_type.is_empty()).then_some(juridiction_type);
    let fields = (!field.is_empty()).then_some(field);
    pipeline::reextract_fields(
        fields.as_deref(),
        overwrite,
        full,
        jts.as_deref(),
        citing_ref_uid.as_deref(),
        workers,
    )
    .await
}

/// Re-fetch ciblé de décisions Judilibre par id puis ingest (ADR 0087).
async fn cmd_refetch(ids: Vec<String>, with_embeddings: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = judilibre_client(&settings)?;
    pipeline::refetch_judilibre(&client, &ids, with_embeddings).await
}

/// Réparation des faux merges judilibre (#29 / ADR 0100 §5). Dry-run (défaut) :
/// read-only, aucun client requis. `--execute` : construit le client Judilibre
/// (re-fetch des provenances divergentes) puis re-split + re-embed ciblé.
async fn cmd_resplit_false_merges(
    execute: bool,
    audit_sample: usize,
    limit: Option<usize>,
) -> Result<()> {
    let dry_run = !execute;
    let settings = Settings::from_env()?;
    let client = if dry_run {
        None
    } else {
        Some(judilibre_client(&settings)?)
    };
    pipeline::resplit_false_merges(client.as_ref(), dry_run, audit_sample, limit).await
}

// ─────────────────────────────────────────────────────────────────────────────
// purge-reverses (port de `cmd_purge_reverses`)
// ─────────────────────────────────────────────────────────────────────────────

/// Hard-delete des décisions retirées (CSV `documents_reverses`), port de
/// `cmd_purge_reverses`.
async fn cmd_purge_reverses() -> Result<()> {
    let settings = Settings::from_env()?;
    let csv_dir = settings
        .cache_dir()
        .join(OPENDATA_SOURCE_DIR)
        .join("documents_reverses");
    if !csv_dir.exists() {
        return Err(anyhow!("Dossier introuvable : {}", csv_dir.display()));
    }
    let pool = pool_with_migrations(&settings).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let summaries = reverses::purge_all(&csv_dir, &repo).await?;
    for s in &summaries {
        println!(
            "{} : {} lignes, {} décisions marquées supprimées, {} absentes ou déjà purgées.",
            s.file, s.processed, s.newly_deleted, s.already_deleted_or_missing
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// generate-summaries (port de `cmd_generate_summaries`)
// ─────────────────────────────────────────────────────────────────────────────

/// Backfill des résumés Mistral (port de `cmd_generate_summaries`). `--concurrency`
/// borne les appels Mistral concurrents (défaut dérivé du nombre de clés) ;
/// `batch_size=1000`, `shuffle=False` reprennent les défauts Typer.
async fn summary_pipeline_generate(limit: Option<usize>, concurrency: Option<usize>) -> Result<()> {
    summary::backfill_summaries(
        lj_core::summary::SUMMARY_PROMPT_VERSION,
        concurrency,
        1000,
        limit.map(|l| l as i64),
        false,
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// sitemap (port de `cmd_sitemap`)
// ─────────────────────────────────────────────────────────────────────────────

/// Source sitemap matérialisée : décisions `(public_id, lastmod)` et articles de
/// référentiel `(slug, num, lastmod)` pré-chargés depuis le repo (les méthodes
/// repo sont async, le trait `SitemapSource` est sync).
struct LoadedSitemapSource {
    decisions: Vec<(String, chrono::NaiveDate)>,
    referential: Vec<(String, String, chrono::NaiveDate)>,
}

impl sitemap::SitemapSource for LoadedSitemapSource {
    fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, chrono::NaiveDate)>> {
        Ok(self.decisions.clone())
    }
    fn iter_referential_for_sitemap(&self) -> Result<Vec<(String, String, chrono::NaiveDate)>> {
        Ok(self.referential.clone())
    }
}

/// Génère les sitemaps et les publie en base (ADR 0064).
///
/// Charge les décisions via le repo, construit l'ensemble en mémoire, puis
/// (sauf `--dry-run`) remplace la table `sitemaps` dans une transaction unique
/// — `lj-server` sert ces lignes sous `/sitemap.xml` + `/sitemap-{n}.xml.gz`.
async fn cmd_sitemap(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;

    tracing::info!("=== build sitemaps ===");
    let pool = pool_with_migrations(&settings).await?;
    let (decisions, referential) = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let decisions = repo
            .iter_decisions_for_sitemap(sitemap::MAX_URLS_PER_SITEMAP as i64)
            .await
            .map_err(|e| anyhow!("iter_decisions_for_sitemap: {e}"))?;
        let referential = repo
            .iter_referential_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_referential_for_sitemap: {e}"))?;
        (decisions, referential)
    };
    let source = LoadedSitemapSource {
        decisions,
        referential,
    };
    let files = sitemap::build_sitemaps(&source)?;

    let sub_count = files.len() - 1; // index exclu
    tracing::info!(
        sub_count,
        index = sitemap::SITEMAP_INDEX_NAME,
        "sitemaps générés"
    );

    if dry_run {
        tracing::info!("--dry-run : pas d'écriture en base.");
        return Ok(());
    }

    // Régénération complète atomique : DELETE + INSERT en une transaction (les
    // sub-sitemaps d'un corpus rétréci disparaissent automatiquement).
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
    tracing::info!(count = rows.len(), "sitemaps publiés en base");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// indexnow (port de `cmd_indexnow`)
// ─────────────────────────────────────────────────────────────────────────────

/// Pousse vers IndexNow les décisions récemment mises à jour (port de
/// `cmd_indexnow`).
///
/// Valide la clé (`LIBREJUSTICE_INDEXNOW_KEY`), collecte les `public_id` dont
/// `updated_at >= now - since_hours` (cap dur à `max_urls`, troncature
/// signalée), puis soumet via `indexnow::submit`.
async fn cmd_indexnow(since_hours: f64, max_urls: usize) -> Result<()> {
    use chrono::{Duration, Utc};

    let settings = Settings::from_env()?;
    let Some(key) = settings.indexnow_key.as_deref() else {
        return Err(anyhow!(
            "LIBREJUSTICE_INDEXNOW_KEY non posé — clé IndexNow requise."
        ));
    };
    // `timedelta(hours=since_hours)` — fenêtre flottante d'heures.
    let since = Utc::now() - Duration::milliseconds((since_hours * 3_600_000.0) as i64);

    let pool = pool_with_migrations(&settings).await?;
    let (public_ids, truncated) = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        // `iter_public_ids_updated_since` pagine en keyset sur `id` mais filtre
        // `updated_at` (aucun index dédié, par choix — on n'alourdit pas le schéma
        // pour IndexNow). Sur un gros backlog (rattrapage, re-résumé en masse) un
        // batch balaie d'immenses plages d'`id` et dépasse le `statement_timeout`
        // 30 s par défaut → `query_canceled`. On le lève sur CETTE connexion : la
        // commande `indexnow` est un process CLI one-shot (pas de réutilisation de
        // la connexion poolée après), donc aucune fuite du SET de session.
        conn.batch_execute("SET statement_timeout = 0").await?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        // `iter_public_ids_updated_since` (batch 10 000 côté Python) matérialise
        // les `public_id`. On applique ensuite le cap dur `max_urls` (troncature
        // jamais muette — règle #12).
        let all = repo
            .iter_public_ids_updated_since(since, 10_000)
            .await
            .map_err(|e| anyhow!("iter_public_ids_updated_since: {e}"))?;
        let truncated = all.len() > max_urls;
        let mut public_ids = all;
        public_ids.truncate(max_urls);
        (public_ids, truncated)
    };

    if truncated {
        // Cap explicite, jamais muet (AGENTS.md règle #12) : on signale la
        // troncature pour ne pas laisser croire à une couverture complète.
        tracing::warn!(
            max_urls,
            since_hours,
            "plus de décisions mises à jour que la borne — soumission tronquée. \
             Relancer avec --max-urls plus haut, ou par fenêtres --since-hours plus courtes."
        );
    }
    let submitted = indexnow::submit(&public_ids, key).await?;
    tracing::info!(submitted, "IndexNow : URL(s) soumises.");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// db (port des `@db_app.command`)
// ─────────────────────────────────────────────────────────────────────────────

/// `statement_timeout` des commandes de maintenance lourdes (ANALYZE, VACUUM
/// FULL, REINDEX CONCURRENTLY, prewarm). `build_pool` arme 30 s sur chaque
/// connexion pour borner les requêtes API/search ; ces ops-là durent
/// légitimement de plusieurs minutes (REINDEX) à ~1 h (VACUUM FULL) et étaient
/// donc **tuées à 30 s** (compaction jamais aboutie → bloat). Un `SET` de
/// session écrase l'option de démarrage `options`. Backstop fini volontaire
/// (pas `0`) : une op qui dérape finit par être tuée.
const MAINTENANCE_STATEMENT_TIMEOUT: &str = "2h";

/// Maintenance DB (port des sous-commandes `db.*`). Toutes ouvrent une
/// connexion en `autocommit` (équivalent `conn.autocommit = True`) et exécutent
/// du SQL de maintenance — pas de migrations.
async fn cmd_db(command: DbCommand) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = pool_only(&settings).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Relève le statement_timeout pour les ops lourdes (cf. constante). `gc-mcp`
    // (DELETE borné) garde le garde-fou 30 s.
    if !matches!(command, DbCommand::GcMcp) {
        conn.batch_execute(&format!(
            "SET statement_timeout = '{MAINTENANCE_STATEMENT_TIMEOUT}'"
        ))
        .await?;
    }
    match command {
        DbCommand::Analyze => db_analyze(&conn).await,
        DbCommand::ReindexSearch => db_reindex_search(&conn).await,
        DbCommand::VacuumFullChunks => db_vacuum_full_chunks(&conn).await,
        DbCommand::GcMcp => db_gc_mcp(&conn).await,
        DbCommand::Prewarm => db_prewarm(&conn).await,
    }
}

/// `ANALYZE` des tables hot (port de `cmd_db_analyze`).
async fn db_analyze(conn: &lj_store::db::Connection) -> Result<()> {
    // `conn.autocommit = True` côté Python : sans transaction ouverte, chaque
    // `execute` tokio-postgres s'auto-commit. ANALYZE ne tourne pas dans un bloc.
    conn.execute("ANALYZE decisions", &[]).await?;
    conn.execute("ANALYZE decision_chunks", &[]).await?;
    conn.execute("ANALYZE decision_full_text", &[]).await?;
    // Citations à plat + catalogue (ADR 0145) : stats fraîches pour l'overlay,
    // les backlinks (`idx_lc_ref`) et les requêtes de service `/loi/`.
    conn.execute("ANALYZE legal_citation", &[]).await?;
    conn.execute("ANALYZE legal_text", &[]).await?;
    conn.execute("ANALYZE legal_article", &[]).await?;
    println!("ANALYZE terminé.");
    Ok(())
}

/// `REINDEX` de `decisions_bm25` puis `chunks_vec` (port de `cmd_db_reindex_search`).
///
/// `decisions_bm25` est l'index BM25 au grain décision (ADR 0084) ; `chunks_bm25`
/// a été droppé. Le clap n'expose pas `--concurrent`/`--maintenance-work-mem`/
/// `--max-parallel-workers` : on reprend les défauts Typer (`concurrent=True`,
/// pas d'override de session). Le pré-flight `DROP INDEX IF EXISTS *_ccnew`
/// (auto-heal d'un REINDEX CONCURRENTLY tué) est conservé.
async fn db_reindex_search(conn: &lj_store::db::Connection) -> Result<()> {
    for index_name in ["decisions_bm25", "chunks_vec"] {
        let orphan = format!("{index_name}_ccnew");
        println!("DROP INDEX IF EXISTS {orphan} …");
        conn.batch_execute(&format!("DROP INDEX IF EXISTS {orphan}"))
            .await?;
    }
    // `concurrent=True` (défaut Typer) → REINDEX INDEX CONCURRENTLY. CONCURRENTLY
    // ne peut pas tourner dans un bloc transactionnel → `batch_execute` (mode
    // simple, hors transaction implicite des requêtes paramétrées).
    for index_name in ["decisions_bm25", "chunks_vec"] {
        println!("REINDEX INDEX CONCURRENTLY {index_name} …");
        conn.batch_execute(&format!("REINDEX INDEX CONCURRENTLY {index_name}"))
            .await?;
        println!("REINDEX {index_name} terminé.");
    }
    Ok(())
}

/// `VACUUM FULL decision_chunks` (port de `cmd_db_vacuum_full_chunks`).
///
/// Le clap n'expose pas `--maintenance-work-mem` : défaut Typer `'512MB'`.
async fn db_vacuum_full_chunks(conn: &lj_store::db::Connection) -> Result<()> {
    let maintenance_work_mem = "512MB";
    conn.batch_execute(&format!(
        "SET maintenance_work_mem = '{maintenance_work_mem}'"
    ))
    .await?;
    println!(
        "VACUUM FULL decision_chunks (mwm={maintenance_work_mem}) … \
         [lock AccessExclusive, /search indisponible]"
    );
    // VACUUM FULL ne peut pas tourner dans un bloc transactionnel.
    conn.batch_execute("VACUUM FULL decision_chunks").await?;
    println!("VACUUM FULL terminé.");
    Ok(())
}

/// Purge des tokens/codes MCP expirés (port de `cmd_db_gc_mcp`).
async fn db_gc_mcp(conn: &lj_store::db::Connection) -> Result<()> {
    let tokens = conn
        .execute("DELETE FROM mcp_tokens WHERE expires_at < now()", &[])
        .await?;
    let codes = conn
        .execute("DELETE FROM mcp_auth_codes WHERE expires_at < now()", &[])
        .await?;
    println!("GC MCP : {tokens} token(s) + {codes} code(s) expirés supprimés.");
    Ok(())
}

/// Prewarm des deux index de recherche : l'ANN `chunks_vec` et le BM25
/// `chunks_bm25`. Sans le BM25, `bm25_parse_leg` paie le coût d'ouverture des
/// segments à froid (mesuré 10-22 s vs 45 ms à chaud). `chunks_bm25` est tiré
/// en mode `read` (cache page OS) plutôt que `buffer` : à 16 Go il dépasse les
/// 5 Go de `shared_buffers`, donc le mode `buffer` éviction­nerait tout le reste.
async fn db_prewarm(conn: &lj_store::db::Connection) -> Result<()> {
    println!("vchordrq_prewarm('chunks_vec', 0) … [tire ~4,2 Go en cache, lent à froid]");
    let row = conn
        .query_opt("SELECT vchordrq_prewarm('chunks_vec', 0)", &[])
        .await?;
    match row {
        Some(row) => {
            // La fonction retourne un entier (nb de buffers chargés).
            let n: i32 = row.try_get(0).unwrap_or(0);
            println!("{n}");
        }
        None => println!("(aucune sortie)"),
    }

    println!("pg_prewarm('decisions_bm25', 'read') … [tire ~12 Go en cache page OS, lent à froid]");
    let row = conn
        .query_opt("SELECT pg_prewarm('decisions_bm25', 'read')", &[])
        .await?;
    match row {
        Some(row) => {
            // `pg_prewarm` retourne le nombre de blocs (int8) chargés.
            let n: i64 = row.try_get(0).unwrap_or(0);
            println!("{n}");
        }
        None => println!("(aucune sortie)"),
    }
    Ok(())
}
