//! Loader générique de corpus de loi curé → `legal_text` + `legal_article`.
//!
//! Source-agnostique et **pur inséreur** : le dataset arrive sous
//! `<state_dir>/ingest/corpus/`, articles déjà découpés et nettoyés, et `texte`
//! est inséré **VERBATIM**. Les seules dérivations sont des clés calculées à
//! partir des champs fournis (`num_key`, `status`, `instrument_key`).
//!
//! ⛔ Aucun nettoyage ni normalisation de texte ici : glyphes PUA, césures,
//! en-têtes, mojibake, reflux de paragraphes se corrigent chez le producteur du
//! dataset, qui recharge. Seul `lj_core::normalizer::normalize_chars`, invariant
//! et commun à toutes les sources, s'applique en aval.
//!
//! Chaque article du dataset est **mono-version** (`texte` : forme consolidée,
//! `effect_date` = `date_debut` commun) ou **multi-versions** (`versions[]` :
//! historique par article, une ligne `legal_article` par version datée).
//!
//! Le dataset est **autoritaire** pour son texte : on purge ses articles avant
//! rechargement. DB uniquement via le repository `lj-store`.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use lj_store::repository::{
    DecisionRepository, LegalArticleRow, LegalLinkOwner, LegalLinkRow, LegalTextRow,
};

use crate::config::Settings;

/// Un texte curé : métadonnées `legal_text` + chemin du markdown OCR à segmenter.
#[derive(Deserialize)]
struct CorpusDoc {
    text_uid: String,
    source: String,
    jurisdiction: String,
    title: String,
    /// Clé de liaison des citations (fold du linker sur `title_key`). Absente, elle
    /// est dérivée du `title` par `normalize_instrument`. Override explicite quand la
    /// forme citée par les juridictions (« code civil guinéen ») diffère du `title`
    /// canonique d'affichage.
    #[serde(default)]
    title_key: Option<String>,
    nature: String,
    /// Provenance structurée du `texte` : `officiel` / `non_officiel` /
    /// `automatique`. Défaut `non_officiel`.
    #[serde(default = "default_translation")]
    translation: String,
    #[serde(default)]
    source_url: Option<String>,
    /// Date « as-of » de fraîcheur, ISO `YYYY-MM-DD` : dernière date à laquelle
    /// cette copie reflète le droit en vigueur. Absente ⇒ le loader pose la date du
    /// run.
    #[serde(default)]
    source_asof: Option<String>,
    /// Source secondaire amont : site qu'un agrégateur pointe. Rare.
    #[serde(default)]
    source_upstream_url: Option<String>,
    /// Date du texte (signature/adoption) si connue, ISO `YYYY-MM-DD`.
    #[serde(default)]
    date_texte: Option<String>,
    /// Date d'effet de la forme consolidée (ISO `YYYY-MM-DD`) : `date_debut` des
    /// articles **mono-version** (law-at-date). Absent ⇒ borne ouverte (sentinelle).
    /// Ignoré pour les articles **multi-versions**, chaque version portant sa propre
    /// `date_debut`.
    #[serde(default)]
    effect_date: Option<String>,
    /// Si vrai, le `status` de chaque article est dérivé du corps (`(Abrogé…)`/vide
    /// ⇒ `ABROGE`, sinon `VIGUEUR`) au lieu d'être forcé `VIGUEUR` — pour les formes
    /// consolidées qui annotent leurs articles abrogés. Défaut : `false`.
    #[serde(default)]
    detect_abrogation: bool,
    /// Si vrai, la résolution d'article de ce texte est PRÉFIXE-AGNOSTIQUE : une
    /// citation matche sur le cœur numérique quel que soit le préfixe d'instrument
    /// (codes territoriaux PF/NC cités « L./D./A./nu » alors que l'officiel est
    /// « A./LP. »). À poser SEULEMENT si le n° identifie l'article à lui seul dans ce
    /// code. Défaut : `false`.
    #[serde(default)]
    num_prefix_agnostic: bool,
    /// Articles déjà segmentés par la curation Python. Chaque article est
    /// **mono-version** (`texte`) ou **multi-versions** (`versions[]`, historique daté).
    articles: Vec<CorpusArticle>,
    /// Arêtes de graphe curées (`legal_link`) possédées par CE texte au niveau
    /// texte, en remplacement-par-propriétaire comme les articles. Réservées aux uid
    /// hors fonds DILA, où les arêtes texte-niveau appartiennent aux fonds.
    #[serde(default)]
    links: Vec<CorpusLink>,
}

/// Une arête curée. `direction` vue du texte propriétaire : `outgoing` = il
/// agit (défaut), `incoming` = il subit — un instrument porte son arête `publie`
/// en `incoming` (« publié par le véhicule »), les fonds ne connaissant jamais
/// son uid.
#[derive(Deserialize)]
struct CorpusLink {
    verb: String,
    #[serde(default = "default_link_direction")]
    direction: String,
    target_text_uid: String,
    /// Libellé affichable de la cible (repris tel quel par les consommateurs
    /// quand la cible n'est pas résolue en base).
    target_label: String,
    /// Date de l'acte cible (ISO `YYYY-MM-DD`), si connue.
    #[serde(default)]
    date: Option<String>,
}

/// Défaut de `CorpusLink::direction` (le propriétaire agit).
fn default_link_direction() -> String {
    "outgoing".to_string()
}

/// Un uid appartenant aux fonds DILA : leurs arêtes texte-niveau sont
/// re-livrées par les syncs (remplacement-par-propriétaire), un dataset curé ne
/// doit jamais posséder d'arêtes sur ces uid.
fn is_dila_uid(uid: &str) -> bool {
    ["JORFTEXT", "LEGITEXT", "KALI", "BOI-"]
        .iter()
        .any(|p| uid.starts_with(p))
}

/// Un article fourni en ligne. Forme **mono-version** (`texte` seul) ou
/// **multi-versions** (`versions[]`) — exclusives.
#[derive(Deserialize)]
struct CorpusArticle {
    num: String,
    /// Mono-version : corps consolidé (jafbase, UE, traités non amendés).
    #[serde(default)]
    texte: Option<String>,
    /// Corps dans la langue d'origine, si disponible — mono-version uniquement.
    #[serde(default)]
    texte_original: Option<String>,
    /// Langue de `texte_original` (ISO-639-1, `ar`…).
    #[serde(default)]
    lang_original: Option<String>,
    /// Override de `translation` pour CET article : une curation fusionne parfois
    /// des provenances de traduction hétérogènes dans un seul texte. Absent ⇒ hérite
    /// de `doc.translation`.
    #[serde(default)]
    translation: Option<String>,
    /// Apparat éditorial affiché construit par la curation : jurisprudence et
    /// doctrine extraites du corps des éditions annotées, renvois « voir aussi » vers
    /// les instruments insérés. Inséré tel quel dans `legal_article.nota`.
    #[serde(default)]
    nota: Option<String>,
    /// Multi-versions : historique daté (avenants des traités). Exclusif avec `texte`.
    #[serde(default)]
    versions: Option<Vec<CorpusVersion>>,
    /// Fil d'Ariane des divisions englobantes, segments joints par ` > ` — même
    /// sérialisation que `legal_article.title_path` côté LEGI. Le loader en dérive
    /// aussi les arêtes `legal_toc_edge`. Pour un article multi-versions, le chemin est
    /// celui de la structure COURANTE, chaque version recevant une arête fenêtrée à sa
    /// place.
    #[serde(default)]
    title_path: Option<String>,
}

/// Défaut de `CorpusDoc::translation` (cas jafbase courant : traduction tierce).
fn default_translation() -> String {
    "non_officiel".to_string()
}

/// Une version datée d'un article. `date_fin = None` ⇒ version courante.
///
/// Provenance **par version** : un traité et ses avenants combinent des sources et
/// des dates distinctes. Chaque champ, absent, retombe sur la valeur doc-level.
/// `source` reste un **libellé de diffuseur** (`legifrance`, `jorf`…), jamais une
/// URL ni une catégorie.
#[derive(Deserialize)]
struct CorpusVersion {
    /// Absente ⇒ borne ouverte (version la plus ancienne dont le début n'est pas
    /// connu). Au plus une par article : la PK `(text_uid, num_key, date_debut)` porte
    /// une sentinelle.
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

/// Le corps s'OUVRE-t-il sur une annonce d'abrogation ? (`(Abrogé par …)`,
/// `Article abrogé`, `Abrogé`, `Implicitement abrogé …`.)
///
/// Le participe reste ACCENTUÉ à dessein — « abroge » nu est la forme verbale d'un
/// article EN VIGUEUR qui en abroge un autre, et l'accepter retournerait le statut
/// de son porteur.
fn opens_on_abrogation(texte: &str) -> bool {
    let low = texte.trim_start().to_lowercase();
    let low = ["implicitement ", "tacitement "]
        .iter()
        .find_map(|adv| low.strip_prefix(adv))
        .unwrap_or(&low);
    low.starts_with("abrogé") || low.starts_with("article abrogé") || low.starts_with("(abrogé")
}

/// Retire UN préfixe éditorial de tête : une référence d'acte parenthésée que
/// les formes consolidées posent avant le corps (« ( L. du 20 juillet 2018 ) »,
/// « (ancien article 30bis) »). `None` si le corps n'en ouvre pas un.
///
/// Groupe non imbriqué et borné : au-delà, ce n'est plus une référence d'acte mais
/// de la prose parenthésée, qu'on ne saute pas.
fn strip_act_reference(texte: &str) -> Option<&str> {
    const MAX_REF: usize = 80;
    let rest = texte.strip_prefix('(')?;
    let close = rest
        .char_indices()
        .take(MAX_REF)
        .find_map(|(i, c)| match c {
            ')' => Some(Some(i)),
            '(' => Some(None),
            _ => None,
        })??;
    Some(rest[close + 1..].trim_start())
}

/// Statut d'un article d'une forme consolidée : `ABROGE` si le corps n'est que
/// l'annonce de son abrogation, ou vide ; `VIGUEUR` sinon.
///
/// L'annonce est cherchée en tête, puis derrière chaque référence d'acte que la
/// consolidation a posée devant elle. La reconnaissance ne peut qu'AJOUTER
/// `ABROGE` : un corps déjà reconnu en tête n'est jamais rouvert, et un reliquat
/// vide n'en fait pas un.
pub(crate) fn article_status(texte: &str) -> &'static str {
    const MAX_PREFIXES: usize = 4;
    let mut t = texte.trim();
    if t.is_empty() {
        return "ABROGE";
    }
    for _ in 0..MAX_PREFIXES {
        if opens_on_abrogation(t) {
            return "ABROGE";
        }
        match strip_act_reference(t) {
            Some(rest) if !rest.is_empty() => t = rest,
            _ => break,
        }
    }
    "VIGUEUR"
}

fn parse_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date {iso:?} invalide: {e}"))
}

/// Date de dernière modification d'un fichier dataset → `NaiveDate` UTC, proxy
/// de fraîcheur « as-of » par défaut.
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

/// Charge les datasets de `<state_dir>/ingest/corpus/*.json`. `only`
/// (sous-chaîne du nom de fichier) restreint le chargement à un lot curé, sans
/// réasserter tous les datasets déjà en base.
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
        tracing::warn!(dir = %dir.display(), "no legal-corpus dataset (.local empty)");
        return Ok(());
    }

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Cibles des arêtes `publie` écrites par ce chargement : candidates au rôle
    //     `vehicule`, classifiées en fin de load.
    let mut publie_targets: std::collections::HashSet<String> = std::collections::HashSet::new();
    for file in &files {
        load_dataset_file(&repo, file, &mut publie_targets).await?;
    }
    run_corpus_finishers(&repo, publie_targets).await
}

/// Charge UN dataset (texte + articles + TOC + arêtes) — corps de boucle du
/// loader, partagé avec `sync-eu-law` qui recharge les seuls datasets régénérés.
/// Accumule les cibles `publie` pour les finisseurs.
pub(crate) async fn load_dataset_file(
    repo: &DecisionRepository<'_>,
    file: &std::path::Path,
    publie_targets: &mut std::collections::HashSet<String>,
) -> Result<()> {
    {
        let raw =
            std::fs::read_to_string(file).with_context(|| format!("lecture {}", file.display()))?;
        let doc: CorpusDoc = serde_json::from_str(&raw)
            .with_context(|| format!("dataset legal-corpus invalide: {}", file.display()))?;
        if lj_norm::jurisdictions::from_url_segment(&doc.jurisdiction.to_ascii_lowercase())
            != Some(doc.jurisdiction.as_str())
        {
            bail!(
                "{}: unknown jurisdiction {:?}",
                file.display(),
                doc.jurisdiction
            );
        }

        let date_texte = doc.date_texte.as_deref().map(parse_date).transpose()?;
        if date_texte.is_none() {
            tracing::warn!(text_uid = %doc.text_uid, dataset = %file.display(), "dataset without date_texte: the text has no date");
        }
        let effect_date = doc.effect_date.as_deref().map(parse_date).transpose()?;
        // Fraîcheur « as-of » : valeur explicite du dataset, ou à défaut la date de
        // modification du FICHIER dataset — la date de curation, invariante au re-load.
        // Posée par article, ici commune au doc.
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
            eli: None,
            nor: None,
            instrument_key: None,
            body: None,
            status: None,
        };
        repo.upsert_legal_text(&trow)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", doc.text_uid))?;
        repo.set_legal_text_num_prefix_agnostic(&doc.text_uid, doc.num_prefix_agnostic)
            .await
            .map_err(|e| anyhow!("set num_prefix_agnostic {}: {e}", doc.text_uid))?;

        // Dataset autoritaire : purge des articles existants avant rechargement.
        let deleted = repo
            .delete_legal_articles_by_text(&doc.text_uid)
            .await
            .map_err(|e| anyhow!("purge {}: {e}", doc.text_uid))?;

        // Articles déjà segmentés par la curation Python. Une ligne par article
        // (mono `texte`) ou par version datée (multi `versions[]`).
        let mut n = 0usize;
        let mut toc_articles: Vec<super::corpus_toc::TocArticle> = Vec::new();
        for (pos, art) in doc.articles.iter().enumerate() {
            // Clé d'identité sans perte, injective par texte : la dé-collision base
            // « 164 » / sous-articles « 164/1 » est intrinsèque.
            let num_key = lj_norm::article_key::identity_key(&art.num);
            match (&art.texte, &art.versions) {
                (Some(_), Some(_)) | (None, None) => bail!(
                    "dataset {} article {} : `texte` (mono) et `versions` (multi) exclusifs, un requis",
                    doc.text_uid,
                    art.num
                ),
                // Multi-versions : une ligne par version datée.
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
                        // Provenance par version : override de l'avenant, sinon doc-level.
                        let v_asof = match v.source_asof.as_deref() {
                            Some(s) => Some(parse_date(s)?),
                            None => source_asof,
                        };
                        // Borne ouverte (pas de début connu) : identité mono-forme, même
                        // `source_uid` que joindrait une arête TOC sans date.
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
                            // Original par version non géré.
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
                        repo.upsert_legal_article(&row, lj_store::repository::Fraicheur::Autoritaire)
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
                    repo.upsert_legal_article(&row, lj_store::repository::Fraicheur::Autoritaire)
                        .await
                        .map_err(|e| anyhow!("upsert {}: {e}", row.source_uid))?;
                    n += 1;
                }
            }
        }
        // Structure : purge autoritaire des arêtes du texte, puis réécriture de
        // l'arbre dérivé des `title_path` (vide = corpus à plat).
        repo.delete_toc_edges_by_text(&doc.text_uid)
            .await
            .map_err(|e| anyhow!("delete_toc_edges_by_text {}: {e}", doc.text_uid))?;
        let toc = super::corpus_toc::derive_corpus_toc(&doc.text_uid, &toc_articles);
        let toc_edges = repo
            .replace_toc_edges(&toc)
            .await
            .map_err(|e| anyhow!("replace_toc_edges {}: {e}", doc.text_uid))?;

        // Arêtes curées : le dataset est autoritaire sur les arêtes texte-niveau de
        // son uid — remplacement-par-propriétaire, un `links` vide purge. Uniquement hors
        // uid DILA : là-bas les arêtes appartiennent aux fonds.
        if is_dila_uid(&doc.text_uid) {
            if !doc.links.is_empty() {
                bail!(
                    "dataset {} : `links[]` interdit sur un uid DILA — les arêtes \
                     texte-niveau y appartiennent aux fonds (ancrer l'instrument \
                     sur un uid propre)",
                    doc.text_uid
                );
            }
        } else {
            let owner = LegalLinkOwner {
                text_uid: doc.text_uid.clone(),
                num_key: String::new(),
                date_debut: None,
            };
            let rows: Vec<LegalLinkRow> = doc
                .links
                .iter()
                .map(|l| {
                    Ok(LegalLinkRow {
                        typelien: l.verb.to_uppercase(),
                        verb: l.verb.clone(),
                        direction: l.direction.clone(),
                        target_kind: "texte".to_string(),
                        target_uid: Some(l.target_text_uid.clone()),
                        target_text_uid: Some(l.target_text_uid.clone()),
                        target_num: None,
                        target_num_key: None,
                        target_nature: None,
                        target_label: l.target_label.clone(),
                        target_date: l.date.as_deref().map(parse_date).transpose()?,
                        target_nor: None,
                    })
                })
                .collect::<Result<_>>()?;
            for r in &rows {
                if r.verb == "publie" && r.direction == "incoming" {
                    if let Some(target) = &r.target_text_uid {
                        publie_targets.insert(target.clone());
                    }
                }
            }
            repo.replace_legal_links(&[(owner, rows)])
                .await
                .map_err(|e| anyhow!("replace_legal_links {}: {e}", doc.text_uid))?;
        }

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
    Ok(())
}

/// Finisseurs de fin de chargement, dans l'ordre des dépendances
/// (`legal_article.searchable` dérive du slug ET du rôle) : slugs, rôles des
/// véhicules déclarés par les datasets, dénorm, puis rangs de lecture. Partagés
/// avec `sync-eu-law`.
pub(crate) async fn run_corpus_finishers(
    repo: &DecisionRepository<'_>,
    publie_targets: std::collections::HashSet<String>,
) -> Result<()> {
    let slugged = super::slugs::assign_text_slugs(repo).await?;
    let uids: Vec<String> = publie_targets.into_iter().collect();
    let (_, _, vehicule, _) = repo
        .classify_text_roles_scoped(&uids)
        .await
        .map_err(|e| anyhow!("classify_text_roles_scoped: {e}"))?;
    let dated = repo
        .date_instruments_from_vehicles(Some(&uids))
        .await
        .map_err(|e| anyhow!("date_instruments_from_vehicles: {e}"))?;
    repo.refresh_article_denorm(None)
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
    // Ordre de lecture des textes touchés : l'écriture n'a fait qu'invalider les
    //     rangs, ce finisseur les recalcule.
    let (ranked_texts, ranked_rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    tracing::info!(ranked_texts, ranked_rows, "reading ranks");
    tracing::info!(
        slugged,
        vehicule,
        dated = dated.len(),
        "legal corpus finishers"
    );
    Ok(())
}

/// Commande `reading-ranks` : recalcule l'ordre de lecture des textes sales.
/// Même appel que le finisseur de fin de pipeline.
pub async fn reading_ranks() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let (texts, rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    tracing::info!(texts, rows, "reading ranks");
    println!("reading-ranks: {texts} texts recomputed, {rows} rows written.");
    Ok(())
}

/// Re-confirme la fraîcheur « as-of » des sources *live* autoritaires
/// (legifrance/kali/jorf) à la date du jour dans `ingest_freshness` : à lancer
/// après chaque ingest LEGI/KALI/JORF. La fraîcheur effective d'un article live se
/// dérive ensuite de `COALESCE(legal_article.source_asof, ingest_freshness[source])`.
pub async fn stamp_freshness() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let today = chrono::Utc::now().date_naive();
    // Seules legifrance/kali sont re-synchronisées quotidiennement. jorf/treaty =
    //     bulk DILA → date de get stockée par ligne à l'ingest, pas ici.
    for source in ["legifrance", "kali"] {
        repo.upsert_ingest_freshness(source, today)
            .await
            .map_err(|e| anyhow!("upsert_ingest_freshness {source}: {e}"))?;
    }
    tracing::info!(asof = %today, "ingest_freshness refreshed (legifrance/kali)");
    Ok(())
}

/// Reclasse en bloc les `source` **catégorie/méthode** hérités (`treaty`,
/// `eu-law`, `official-fr`, `traduction-automatique`) en libellés de **diffuseur**
/// réels :
/// 1. lignes portant une URL → libellé dérivé de `source_url`
///    ([`lj_norm::source_authority::diffuseur_label_from_url`], source de vérité
///    partagée) ;
/// 2. traités du **bulk JORF** (sans URL, `source_uid=JORFARTI…`) → `jorf` ;
/// 3. reliquat sans URL hors bulk (chaînes curées de traités, vieux codes
///    traduits) : laissé au reload de curation, **compté et logué**, jamais
///    réécrit en aveugle.
///
/// Idempotent : un re-run ne trouve plus de catégorie à reclasser.
pub async fn relabel_sources() -> Result<()> {
    const CATEGORIES: &[&str] = &["treaty", "eu-law", "official-fr", "traduction-automatique"];

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // 1. Lignes avec URL : libellé canonique dérivé de l'URL.
    let pairs = repo
        .distinct_category_source_urls(CATEGORIES)
        .await
        .map_err(|e| anyhow!("distinct_category_source_urls: {e}"))?;
    let mut relabelled = 0u64;
    for (old, url) in &pairs {
        let new = lj_norm::source_authority::diffuseur_label_from_url(url)
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

    // Reclasse en bloc les `source` catégorie/méthode hérités en libellés de
    // **diffuseur** réels :
    // 1. lignes portant une URL → libellé dérivé de `source_url`
    //    ([`lj_norm::source_authority::diffuseur_label_from_url`]) ;
    // 2. traités du bulk JORF (sans URL, `source_uid=JORFARTI…`) → `jorf` ;
    // 3. reliquat sans URL hors bulk : laissé au reload de curation, compté et logué,
    //    jamais réécrit en aveugle.
    //
    // Idempotent.
    let leftovers = repo
        .count_category_source_leftovers(CATEGORIES)
        .await
        .map_err(|e| anyhow!("count_category_source_leftovers: {e}"))?;
    for (src, n) in &leftovers {
        tracing::warn!(
            source = %src,
            rows = n,
            "source category left over (empty URL), fix it with a curation reload"
        );
    }
    tracing::info!(
        relabelled,
        jorf_bulk,
        leftovers = leftovers.iter().map(|(_, n)| n).sum::<i64>(),
        "relabel-sources done"
    );
    Ok(())
}

/// Canonicalise les **variantes d'hôte** d'un même diffuseur vers son libellé
/// unique — la dérivation canonique vit dans
/// [`lj_norm::source_authority::diffuseur_label_from_url`], cette commande rabat
/// les lignes DÉJÀ stockées. Idempotent. À étendre si d'autres variantes d'hôte
/// apparaissent.
pub async fn canonicalize_source_labels() -> Result<()> {
    // Variante d'hôte → libellé canonique.
    const CANON: &[(&str, &str)] = &[("jafbase.fr", "jafbase")];

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut total = 0u64;
    for (from, to) in CANON {
        let n = repo
            .canonicalize_source_label(from, to)
            .await
            .map_err(|e| anyhow!("canonicalize_source_label {from}→{to}: {e}"))?;
        tracing::info!(from = %from, to = %to, rows = n, "canonicalising the publisher label");
        total += n;
    }
    println!("canonicalize-source-labels: {total} rows folded onto their canonical label.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::article_status;

    #[test]
    fn article_status_detecte_abrogation() {
        assert_eq!(article_status("Article abrogé"), "ABROGE");
        assert_eq!(article_status("  Article abrogé\n"), "ABROGE");
        assert_eq!(article_status("Abrogé"), "ABROGE");
        assert_eq!(
            article_status("(Abrogé par la loi du pays n° 2019-12)"),
            "ABROGE"
        );
        assert_eq!(article_status(""), "ABROGE");
        assert_eq!(article_status("Le recouvrement des impôts…"), "VIGUEUR");
    }

    #[test]
    fn article_status_saute_la_reference_d_acte_de_tete() {
        assert_eq!(article_status("Abrogé ( L. du 27 juin 2018 )"), "ABROGE");
        assert_eq!(
            article_status("( L. du 20 juillet 2018 ) Article abrogé."),
            "ABROGE"
        );
        assert_eq!(article_status("( L. du 27 juin 2018 )\nAbrogés"), "ABROGE");
        assert_eq!(article_status("(ancien article 30bis) (Abrogé)"), "ABROGE");
        assert_eq!(
            article_status("Implicitement abrogé ( L. 13 juin 1994 )."),
            "ABROGE"
        );
        assert_eq!(
            article_status("Implicitement abrogé (C. pén., art. 387)"),
            "ABROGE"
        );
    }

    #[test]
    fn article_status_ne_bascule_pas_sur_un_article_vivant() {
        assert_eq!(
            article_status("(supprimé par changement de numérotation)"),
            "VIGUEUR"
        );
        assert_eq!(
            article_status("Abroge l'article 5 de la loi du 3 juillet 1978."),
            "VIGUEUR"
        );
        assert_eq!(
            article_status(
                "( L. 12 août 2003 ) Les dispositions abrogées par la loi \
                            du 4 juillet 2014 restent applicables aux instances en cours."
            ),
            "VIGUEUR"
        );
        assert_eq!(
            article_status(
                "(Le présent article, dont la rédaction résulte d'une refonte complète \
                 opérée en 2018, régit la matière) Abrogé"
            ),
            "VIGUEUR"
        );
    }
}
