//! Loader générique de corpus de loi curé → `legal_text` + `legal_article`.
//!
//! Source-agnostique (ADR 0108/0118) : la moitié **adaptative** (OCR, choix du
//! document, juridiction/nature/statut, **segmentation**) vit en **Python jettable**
//! (`scripts/segment_jafbase.py`, `scripts/curate_*.py`) qui produit un dataset JSON
//! par texte sous `<state_dir>/ingest/corpus/`, **articles déjà découpés ET
//! déjà nettoyés** (ADR 0118). Ce module n'est qu'un **pur inséreur** : il upsert le
//! texte et insère les articles fournis, `texte` inséré **VERBATIM**. Les seules
//! dérivations tolérées sont des CLÉS/colonnes calculées à partir des champs fournis
//! (`num_key = normalize_article`, invariant partagé avec la résolution des citations ;
//! `status`/`instrument_key`) — jamais une modification du contenu.
//!
//! ⛔ RÈGLE ABSOLUE — AUCUN NETTOYAGE / NORMALISATION DE TEXTE DANS CE FICHIER. Tout
//! traitement du CORPS (glyphes PUA Symbol/Wingdings, césures, en-têtes/pieds de page,
//! mojibake, cruft de navigation, reflux de paragraphes, `replace`/`sanitize`/`strip`/
//! regex sur `texte`…) se fait EXCLUSIVEMENT dans les scripts **Python jettables**
//! (`scripts/curate_*.py`, `scripts/segment_jafbase.py`, `scripts/lj_segment.py`,
//! `scripts/clean_pua_cruft.py`), JAMAIS ici. Ce loader Rust d'ingest doit rester
//! EXTRÊMEMENT LÉGER. Toute logique de cleaning ajoutée ici est un bug : la corriger
//! côté producteur Python et recharger. De même, aucune commande de nettoyage in-place
//! (SQL) ne vit dans ce binaire — c'est un script Python jettable.
//!
//! Chaque article du dataset est, exclusivement :
//! - **mono-version** (`texte`) : forme consolidée, `effect_date` = `date_debut`
//!   commun (law-at-date). Cas par défaut (jafbase, UE, traités non amendés).
//! - **multi-versions** (`versions[]`) : historique par article (avenants des
//!   traités), une ligne `legal_article` par version datée.
//!
//! Le dataset est **autoritaire** pour son texte : on purge ses articles avant
//! rechargement (idempotence #7). DB uniquement via le repository `lj-store`
//! (règle #2) — ce loader est la frontière d'écriture, pas le Python.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use lj_store::repository::{DecisionRepository, LegalArticleRow, LegalTextRow};

use crate::config::Settings;

/// Un texte curé : métadonnées `legal_text` + chemin du markdown OCR à segmenter.
#[derive(Deserialize)]
struct CorpusDoc {
    text_uid: String,
    source: String,
    jurisdiction: String,
    title: String,
    /// Clé de liaison des citations (fold du linker sur `title_key`, ADR 0145).
    /// Si absente, dérivée du `title` par `normalize_instrument` (cas courant). Override
    /// explicite quand la **forme citée** par les juridictions (gentilé « code civil guinéen »,
    /// nom tronqué « loi fédérale suisse sur la poursuite ») diffère du `title` canonique
    /// d'affichage — découple liaison et affichage (cf. working-note title_key).
    #[serde(default)]
    title_key: Option<String>,
    nature: String,
    /// Provenance structurée du `texte` (ADR 0116) : `officiel` / `non_officiel` /
    /// `automatique`. Défaut `non_officiel` (cas jafbase courant).
    #[serde(default = "default_translation")]
    translation: String,
    #[serde(default)]
    source_url: Option<String>,
    /// Date « as-of » de fraîcheur (ADR 0129), ISO `YYYY-MM-DD` : dernière base crédible
    /// que cette copie reflète le droit en vigueur. Absente ⇒ le loader pose la date du
    /// run (date de chargement = proxy d'extraction pour le curé).
    #[serde(default)]
    source_asof: Option<String>,
    /// Source secondaire amont (ADR 0129) : site qu'un agrégateur (jafbase) pointe. Rare.
    #[serde(default)]
    source_upstream_url: Option<String>,
    /// Date du texte (signature/adoption) si connue, ISO `YYYY-MM-DD`.
    #[serde(default)]
    date_texte: Option<String>,
    /// Date d'effet de la forme consolidée (ISO `YYYY-MM-DD`) : `date_debut` des
    /// articles **mono-version** (law-at-date). Absent (jafbase) ⇒ borne ouverte
    /// (sentinelle). Ignoré pour les articles **multi-versions** (chaque version
    /// porte sa propre `date_debut`).
    #[serde(default)]
    effect_date: Option<String>,
    /// Si vrai, le `status` de chaque article est dérivé du corps (`(Abrogé…)`/vide ⇒
    /// `ABROGE`, sinon `VIGUEUR`) au lieu d'être forcé `VIGUEUR` — pour les formes
    /// consolidées qui annotent leurs articles abrogés (traités). Défaut: `false`.
    #[serde(default)]
    detect_abrogation: bool,
    /// Si vrai, la résolution d'article de ce texte est PRÉFIXE-AGNOSTIQUE (migration
    /// 0087, §7.4) : une citation matche sur le cœur numérique quel que soit le préfixe
    /// d'instrument (codes territoriaux PF/NC cités « L./D./A./nu » alors que l'officiel
    /// est « A./LP. »). À poser SEULEMENT si le n° identifie l'article à lui seul dans
    /// ce code (aucun n° sous deux préfixes — garanti par le curateur). Défaut: `false`.
    #[serde(default)]
    num_prefix_agnostic: bool,
    /// Articles déjà segmentés par la curation Python (ADR 0118 : le Python segmente,
    /// le loader insère). Chaque article est **mono-version** (`texte`) ou
    /// **multi-versions** (`versions[]`, historique daté des avenants de traités).
    articles: Vec<CorpusArticle>,
}

/// Un article fourni en ligne. Forme **mono-version** (`texte` seul) ou
/// **multi-versions** (`versions[]`) — exclusives.
#[derive(Deserialize)]
struct CorpusArticle {
    num: String,
    /// Mono-version : corps consolidé (jafbase, UE, traités non amendés).
    #[serde(default)]
    texte: Option<String>,
    /// Corps dans la langue d'origine (ADR 0116), si disponible — mono-version
    /// uniquement (couche front/vérification, souvent absent).
    #[serde(default)]
    texte_original: Option<String>,
    /// Langue de `texte_original` (ISO-639-1, `ar`…).
    #[serde(default)]
    lang_original: Option<String>,
    /// Override de `translation` (ADR 0116) pour CET article — la curation fusionne
    /// des provenances de traduction hétérogènes dans un seul texte (ex. code civil
    /// syrien : corps auto-traduit `automatique` + 130 articles à traduction humaine
    /// publiée `non_officiel`). Absent ⇒ hérite de `doc.translation`. Le loader reste
    /// pur inséreur (ADR 0118) : il pose la valeur fournie, il ne la fabrique pas.
    #[serde(default)]
    translation: Option<String>,
    /// Apparat éditorial affiché (ADR 0135) construit par la curation : jurisprudence/
    /// doctrine extraite du corps des éditions annotées, renvois « voir aussi » vers les
    /// instruments insérés. Inséré tel quel dans `legal_article.nota` — le loader reste
    /// un pur inséreur (ADR 0118), aucune fabrication de note côté Rust.
    #[serde(default)]
    nota: Option<String>,
    /// Multi-versions : historique daté (avenants des traités). Exclusif avec `texte`.
    #[serde(default)]
    versions: Option<Vec<CorpusVersion>>,
    /// Fil d'Ariane des divisions englobantes (ADR 0186), segments joints par
    /// ` > ` — même sérialisation que `legal_article.title_path` côté LEGI.
    /// Produit par la curation ; le loader en dérive aussi les arêtes
    /// `legal_toc_edge` (sommaire arborescent + vue-lecture par division).
    /// Pour un article multi-versions (ADR 0187), le chemin est celui de la
    /// structure COURANTE — chaque version reçoit une arête fenêtrée à sa place.
    #[serde(default)]
    title_path: Option<String>,
}

/// Défaut de `CorpusDoc::translation` (cas jafbase courant : traduction tierce).
fn default_translation() -> String {
    "non_officiel".to_string()
}

/// Une version datée d'un article. `date_fin = None` ⇒ version courante.
///
/// Provenance **par version** (ADR 0131) : un traité et ses avenants combinent des
/// sources/dates distinctes (chaque avenant = un décret JORF de publication). Chaque
/// champ, absent, retombe sur la valeur doc-level. `source` reste un **libellé de
/// diffuseur** (`legifrance`, `jorf`…), jamais une URL ni une catégorie.
#[derive(Deserialize)]
struct CorpusVersion {
    /// Absente ⇒ borne ouverte (version la plus ancienne dont le début n'est
    /// pas connu — historique reconstitué d'instantanés, ADR 0187). Au plus
    /// une par article (la PK `(text_uid, num_key, date_debut)` sentinelle).
    #[serde(default)]
    date_debut: Option<String>,
    #[serde(default)]
    date_fin: Option<String>,
    status: String,
    texte: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    /// Fraîcheur « as-of » propre à la version (ISO `YYYY-MM-DD`). Absente ⇒ doc-level.
    #[serde(default)]
    source_asof: Option<String>,
    #[serde(default)]
    source_upstream_url: Option<String>,
}

/// Statut d'un article d'une forme consolidée : `ABROGE` si le corps n'est que
/// l'annotation d'abrogation (`(Abrogé par …)`, `Article abrogé` Lexpol, `Abrogé`)
/// ou vide ; `VIGUEUR` sinon.
fn article_status(texte: &str) -> &'static str {
    let t = texte.trim_start();
    let low = t.to_lowercase();
    if t.is_empty()
        || low.starts_with("(abrogé")
        || low.starts_with("article abrogé")
        || low.starts_with("abrogé")
    {
        "ABROGE"
    } else {
        "VIGUEUR"
    }
}

fn parse_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date {iso:?} invalide: {e}"))
}

/// Date de dernière modification d'un fichier dataset → `NaiveDate` UTC (proxy de
/// fraîcheur « as-of » par défaut, ADR 0129).
fn file_mtime_date(path: &std::path::Path) -> Result<chrono::NaiveDate> {
    let mtime = std::fs::metadata(path)
        .with_context(|| format!("metadata {}", path.display()))?
        .modified()
        .with_context(|| format!("mtime {}", path.display()))?;
    Ok(chrono::DateTime::<chrono::Utc>::from(mtime).date_naive())
}

fn dataset_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("lecture {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    Ok(files)
}

/// Charge les datasets de `<state_dir>/ingest/corpus/*.json`. `only` (sous-chaîne
/// du nom de fichier) restreint le chargement — chargement chirurgical d'un lot curé
/// (ex. `eu-rproc`) sans réasserter les 3800 datasets déjà en base.
pub async fn load_legal_corpus(only: Option<&str>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    let mut files = dataset_files(&dir)?;
    if let Some(pat) = only {
        files.retain(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(pat))
        });
    }
    if files.is_empty() {
        tracing::warn!(dir = %dir.display(), "aucun dataset legal-corpus (.local vide)");
        return Ok(());
    }

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    for file in &files {
        let raw =
            std::fs::read_to_string(file).with_context(|| format!("lecture {}", file.display()))?;
        let doc: CorpusDoc = serde_json::from_str(&raw)
            .with_context(|| format!("dataset legal-corpus invalide: {}", file.display()))?;

        let date_texte = doc.date_texte.as_deref().map(parse_date).transpose()?;
        let effect_date = doc.effect_date.as_deref().map(parse_date).transpose()?;
        // Fraîcheur « as-of » (ADR 0129) : valeur explicite du dataset, ou — à défaut — la
        // date de modification du FICHIER dataset (= date de curation, proxy stable et plus
        // honnête que la date de chargement : invariant au re-load). Posée par article (les
        // avenants de traités peuvent porter des sources/dates distinctes — ici commune au doc).
        let source_asof = match doc.source_asof.as_deref() {
            Some(s) => Some(parse_date(s)?),
            None => Some(file_mtime_date(file)?),
        };
        let trow = LegalTextRow {
            text_uid: doc.text_uid.clone(),
            jurisdiction: doc.jurisdiction.clone(),
            title: doc.title.clone(),
            title_key: doc
                .title_key
                .clone()
                .unwrap_or_else(|| lj_extract::extract::normalize_instrument(&doc.title)),
            nature: doc.nature.clone(),
            last_modified: None,
            date_texte,
            date_publi: None,
            // Droit étranger curé : identité par `text_uid`, cascade ADR 0115 non peuplée.
            eli: None,
            nor: None,
            instrument_key: None,
            body: None,
            status: None,
        };
        repo.upsert_legal_text(&trow)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", doc.text_uid))?;
        // Résolution préfixe-agnostique du texte (migration 0087) : posée à part de
        // l'upsert (qui n'élargit pas `LegalTextRow` à tous ses sites de construction).
        repo.set_legal_text_num_prefix_agnostic(&doc.text_uid, doc.num_prefix_agnostic)
            .await
            .map_err(|e| anyhow!("set num_prefix_agnostic {}: {e}", doc.text_uid))?;

        // Dataset autoritaire : purge des articles existants avant rechargement.
        let deleted = repo
            .delete_legal_articles_by_text(&doc.text_uid)
            .await
            .map_err(|e| anyhow!("purge {}: {e}", doc.text_uid))?;

        // Articles déjà segmentés par la curation Python (ADR 0118). Une ligne par
        // article (mono `texte`) ou par version datée (multi `versions[]`).
        let mut n = 0usize;
        let mut toc_articles: Vec<super::corpus_toc::TocArticle> = Vec::new();
        for (pos, art) in doc.articles.iter().enumerate() {
            // Clé d'identité (ADR 0236) : sans perte, injective par texte — la
            // dé-collision base « 164 » / sous-articles « 164/1 » est intrinsèque.
            let num_key = lj_core::article_key::identity_key(&art.num);
            match (&art.texte, &art.versions) {
                (Some(_), Some(_)) | (None, None) => bail!(
                    "dataset {} article {} : `texte` (mono) et `versions` (multi) exclusifs, un requis",
                    doc.text_uid,
                    art.num
                ),
                // Multi-versions : une ligne par version datée (avenants des
                // traités, historiques reconstitués ADR 0187).
                (None, Some(versions)) => {
                    if versions.iter().filter(|v| v.date_debut.is_none()).count() > 1 {
                        bail!(
                            "dataset {} article {} : plusieurs versions à borne ouverte \
                             (PK sentinelle (text_uid, num_key, date_debut))",
                            doc.text_uid,
                            art.num
                        );
                    }
                    let mut toc_versions = Vec::with_capacity(versions.len());
                    for v in versions {
                        // Provenance par version (ADR 0131) : override de l'avenant, sinon doc-level.
                        let v_asof = match v.source_asof.as_deref() {
                            Some(s) => Some(parse_date(s)?),
                            None => source_asof,
                        };
                        // Borne ouverte (pas de début connu) : identité mono-forme,
                        // même `source_uid` que joindrait une arête TOC sans date.
                        let source_uid = match v.date_debut.as_deref() {
                            Some(d) => format!("{}#{}@{}", doc.text_uid, num_key, d),
                            None => format!("{}#{}", doc.text_uid, num_key),
                        };
                        let date_debut = v.date_debut.as_deref().map(parse_date).transpose()?;
                        let date_fin = v.date_fin.as_deref().map(parse_date).transpose()?;
                        toc_versions.push(super::corpus_toc::TocVersion {
                            source_uid: source_uid.clone(),
                            status: v.status.clone(),
                            date_debut,
                            date_fin,
                        });
                        let row = LegalArticleRow {
                            text_uid: doc.text_uid.clone(),
                            num: art.num.clone(),
                            num_key: num_key.clone(),
                            position: Some(pos as i32),
                            title_path: art.title_path.clone(),
                            status: v.status.clone(),
                            date_debut,
                            date_fin,
                            texte: Some(v.texte.clone()),
                            // Original par version non géré (avenants traités) ; ADR 0116.
                            texte_original: None,
                            lang_original: None,
                            translation: art.translation.clone().unwrap_or_else(|| doc.translation.clone()),
                            nota: art.nota.clone(),
                            content_checksum: xxhash_rust::xxh3::xxh3_64(v.texte.as_bytes()),
                            source: v.source.clone().unwrap_or_else(|| doc.source.clone()),
                            source_uid,
                            source_url: v.source_url.clone().or_else(|| doc.source_url.clone()),
                            source_asof: v_asof,
                            source_upstream_url: v
                                .source_upstream_url
                                .clone()
                                .or_else(|| doc.source_upstream_url.clone()),
                        };
                        repo.upsert_legal_article(&row)
                            .await
                            .map_err(|e| anyhow!("upsert {}: {e}", row.source_uid))?;
                        n += 1;
                    }
                    toc_articles.push(super::corpus_toc::TocArticle {
                        num: art.num.clone(),
                        num_key: num_key.clone(),
                        versions: toc_versions,
                        title_path: art.title_path.clone(),
                    });
                }
                // Mono-version : `effect_date` partagé (law-at-date) ou borne ouverte.
                (Some(texte), None) => {
                    let status = if doc.detect_abrogation {
                        article_status(texte).to_string()
                    } else {
                        "VIGUEUR".to_string()
                    };
                    toc_articles.push(super::corpus_toc::TocArticle {
                        num: art.num.clone(),
                        num_key: num_key.clone(),
                        versions: vec![super::corpus_toc::TocVersion {
                            source_uid: format!("{}#{}", doc.text_uid, num_key),
                            status: status.clone(),
                            date_debut: None,
                            date_fin: None,
                        }],
                        title_path: art.title_path.clone(),
                    });
                    let row = LegalArticleRow {
                        text_uid: doc.text_uid.clone(),
                        num: art.num.clone(),
                        num_key: num_key.clone(),
                        position: Some(pos as i32),
                        title_path: art.title_path.clone(),
                        status,
                        date_debut: effect_date,
                        date_fin: None,
                        texte: Some(texte.clone()),
                        texte_original: art.texte_original.clone(),
                        lang_original: art.lang_original.clone(),
                        translation: art.translation.clone().unwrap_or_else(|| doc.translation.clone()),
                        nota: art.nota.clone(),
                        content_checksum: xxhash_rust::xxh3::xxh3_64(texte.as_bytes()),
                        source: doc.source.clone(),
                        source_uid: format!("{}#{}", doc.text_uid, num_key),
                        source_url: doc.source_url.clone(),
                        source_asof,
                        source_upstream_url: doc.source_upstream_url.clone(),
                    };
                    repo.upsert_legal_article(&row)
                        .await
                        .map_err(|e| anyhow!("upsert {}: {e}", row.source_uid))?;
                    n += 1;
                }
            }
        }
        // Structure (ADR 0186) : purge autoritaire des arêtes du texte, puis
        // réécriture de l'arbre dérivé des `title_path` (vide = corpus à plat).
        repo.delete_toc_edges_by_text(&doc.text_uid)
            .await
            .map_err(|e| anyhow!("delete_toc_edges_by_text {}: {e}", doc.text_uid))?;
        let toc = super::corpus_toc::derive_corpus_toc(&doc.text_uid, &toc_articles);
        let toc_edges = repo
            .replace_toc_edges(&toc)
            .await
            .map_err(|e| anyhow!("replace_toc_edges {}: {e}", doc.text_uid))?;

        tracing::info!(
            text_uid = doc.text_uid,
            source = doc.source,
            jurisdiction = doc.jurisdiction,
            articles = n,
            deleted,
            toc_edges,
            "legal corpus loaded"
        );
    }
    // Titre du texte dénormalisé → `search_title` (ADR 0114).
    repo.refresh_article_denorm()
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(slugged, "legal corpus slugs");
    Ok(())
}

/// Re-confirme la fraîcheur « as-of » des sources *live* autoritaires
/// (legifrance/kali/jorf) à la date du jour dans `ingest_freshness` (ADR 0129) : à
/// lancer après chaque ingest LEGI/KALI/JORF. La fraîcheur effective d'un article live
/// se dérive ensuite de `COALESCE(legal_article.source_asof, ingest_freshness[source])`.
pub async fn stamp_freshness() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let today = chrono::Utc::now().date_naive();
    // Seules legifrance/kali sont re-synchronisées quotidiennement (cron). jorf/treaty =
    // bulk DILA → date de get stockée par ligne à l'ingest (pas ici). ADR 0129.
    for source in ["legifrance", "kali"] {
        repo.upsert_ingest_freshness(source, today)
            .await
            .map_err(|e| anyhow!("upsert_ingest_freshness {source}: {e}"))?;
    }
    tracing::info!(asof = %today, "ingest_freshness rafraîchie (legifrance/kali)");
    Ok(())
}

/// Retrofit ADR 0131 : reclasse en bloc les `source` **catégorie/méthode** hérités
/// (`treaty`, `eu-law`, `official-fr`, `traduction-automatique`) en libellés de
/// **diffuseur** réels. Stratégie :
/// 1. lignes portant une URL → libellé dérivé de `source_url`
///    ([`lj_core::source_authority::diffuseur_label_from_url`], source de vérité partagée) ;
/// 2. traités du **bulk JORF** (sans URL, `source_uid=JORFARTI…`) → `jorf` (DILA) ;
/// 3. reliquat sans URL hors bulk (chaînes curées de traités, vieux codes traduits) :
///    laissé au reload de curation, **compté et logué** (jamais réécrit en aveugle).
///
/// Idempotent (un re-run ne trouve plus de catégorie à reclasser).
pub async fn relabel_sources() -> Result<()> {
    const CATEGORIES: &[&str] = &["treaty", "eu-law", "official-fr", "traduction-automatique"];

    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // 1. Lignes avec URL : libellé canonique dérivé de l'URL.
    let pairs = repo
        .distinct_category_source_urls(CATEGORIES)
        .await
        .map_err(|e| anyhow!("distinct_category_source_urls: {e}"))?;
    let mut relabelled = 0u64;
    for (old, url) in &pairs {
        let new = lj_core::source_authority::diffuseur_label_from_url(url)
            .ok_or_else(|| anyhow!("source_url sans hôte exploitable: {url:?}"))?;
        let n = repo
            .relabel_source_by_url(old, url, &new)
            .await
            .map_err(|e| anyhow!("relabel_source_by_url {old}→{new}: {e}"))?;
        tracing::info!(old = %old, new = %new, url = %url, rows = n, "relabel source (URL)");
        relabelled += n;
    }

    // 2. Traités du bulk JORF (sans URL) → diffuseur 'jorf'.
    let jorf_bulk = repo
        .relabel_treaty_jorf_bulk()
        .await
        .map_err(|e| anyhow!("relabel_treaty_jorf_bulk: {e}"))?;
    tracing::info!(rows = jorf_bulk, "relabel treaty bulk JORF → jorf");

    // 3. Reliquat (URL vide, hors bulk) : à corriger par reload de curation.
    let leftovers = repo
        .count_category_source_leftovers(CATEGORIES)
        .await
        .map_err(|e| anyhow!("count_category_source_leftovers: {e}"))?;
    for (src, n) in &leftovers {
        tracing::warn!(
            source = %src,
            rows = n,
            "source catégorie restante (URL vide) — corriger par reload de curation"
        );
    }
    tracing::info!(
        relabelled,
        jorf_bulk,
        leftovers = leftovers.iter().map(|(_, n)| n).sum::<i64>(),
        "relabel-sources terminé"
    );
    Ok(())
}

/// Canonicalise les **variantes d'hôte** d'un même diffuseur vers son libellé unique
/// (ADR 0131 « un libellé par diffuseur ») — la dérivation canonique vit dans
/// [`lj_core::source_authority::diffuseur_label_from_url`] (corrigée pour les futurs
/// loads) ; cette commande rabat les lignes DÉJÀ stockées. Idempotent. One-shot
/// post-déploiement. À étendre si d'autres variantes d'hôte apparaissent.
pub async fn canonicalize_source_labels() -> Result<()> {
    // (variante d'hôte → libellé canonique). jafbase a deux hôtes (jafbase.fr).
    const CANON: &[(&str, &str)] = &[("jafbase.fr", "jafbase")];

    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut total = 0u64;
    for (from, to) in CANON {
        let n = repo
            .canonicalize_source_label(from, to)
            .await
            .map_err(|e| anyhow!("canonicalize_source_label {from}→{to}: {e}"))?;
        tracing::info!(from = %from, to = %to, rows = n, "canonicalise libellé diffuseur");
        total += n;
    }
    println!("canonicalize-source-labels : {total} lignes rabattues sur leur libellé canonique.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::article_status;

    #[test]
    fn article_status_detecte_abrogation() {
        // Forme Lexpol (codes PF/NC consolidés) + formes traités + vide.
        assert_eq!(article_status("Article abrogé"), "ABROGE");
        assert_eq!(article_status("  Article abrogé\n"), "ABROGE");
        assert_eq!(article_status("Abrogé"), "ABROGE");
        assert_eq!(
            article_status("(Abrogé par la loi du pays n° 2019-12)"),
            "ABROGE"
        );
        assert_eq!(article_status(""), "ABROGE");
        // Texte réel en vigueur.
        assert_eq!(article_status("Le recouvrement des impôts…"), "VIGUEUR");
    }
}
