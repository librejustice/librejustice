//! JORF (Journal officiel, bulk DILA) : ingest tarball stock/incrément du fond
//! complet → référentiel versionné `referential_texts`/`referential_articles`.
//!
//! Structure XML de LEGI, d'où le parser pur [`lj_extract::jorf`]. Trois
//! divergences pilotent ce pipeline :
//! - **Deux passes**, textes puis articles. La passe articles ne persiste que
//!   les `JORFARTI` d'un véhicule cible d'une arête `publie`, routés vers la
//!   fiche instrument, et seulement les annexes (`num` non numérique). Le reste
//!   du JO n'entre pas au référentiel versionné : LEGI consolide les décrets.
//! - **Unnumbered articles stay out of the versioned referential**: an empty
//!   `NUM` is not citable. Their text becomes the body of a JORFTEXT with
//!   neither body nor article, often all an individual act carries.
//! - **Deux séries d'articles dans un même JORFTEXT**, quand une annexe
//!   renumérote à partir de 1 : la passe textes lit les `texte/struct` pour
//!   connaître le dispositif propre de chaque texte, et la passe articles
//!   qualifie par sa section la clé de qui rejoue un de ses numéros.
//!
//! Le corps des accords antérieurs à la numérisation est absent du bulk : leurs
//! `referential_articles` viennent d'un track OCR distinct.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Taille de batch des upserts JORF (textes puis articles).
const JORF_BATCH_SIZE: usize = 512;

/// The kind of a JORF member, read from its tar path; `eli/` is not read.
pub(super) enum JorfMember {
    Article,
    Texte,
    Struct,
    Section,
    Issue,
    Ignore,
}

pub(super) fn classify_jorf_member(name: &str) -> JorfMember {
    let lower = name.to_lowercase();
    let stem = name.rsplit('/').next().unwrap_or(name);
    if lower.contains("/texte/struct/") && stem.starts_with("JORFTEXT") {
        return JorfMember::Struct;
    }
    if lower.contains("/conteneur/") && stem.starts_with("JORFCONT") {
        return JorfMember::Issue;
    }
    if lower.contains("/section_ta/") && stem.starts_with("JORFSCTA") {
        return JorfMember::Section;
    }
    if lower.contains("/struct/") || lower.contains("/eli/") {
        return JorfMember::Ignore;
    }
    if lower.contains("/article/") && stem.starts_with("JORFARTI") {
        return JorfMember::Article;
    }
    if lower.contains("/texte/version/") && stem.starts_with("JORFTEXT") {
        return JorfMember::Texte;
    }
    JorfMember::Ignore
}

/// Unnumbered article text of each JORFTEXT, keyed by `JORFARTI`: a later
/// archive's version replaces the earlier one, and id order is reading order.
#[derive(Default)]
pub(crate) struct UnnumberedBodies(HashMap<String, BTreeMap<String, String>>);

impl UnnumberedBodies {
    fn push(&mut self, jorftext: String, jorfarti: String, texte: String) {
        self.0.entry(jorftext).or_default().insert(jorfarti, texte);
    }

    /// Writes the joined bodies on the empty texts, returns the rows written.
    async fn write(self, repo: &DecisionRepository<'_>) -> Result<u64> {
        let bodies: Vec<(String, String)> = self
            .0
            .into_iter()
            .map(|(uid, arts)| (uid, arts.into_values().collect::<Vec<_>>().join("\n\n")))
            .collect();
        let mut written = 0;
        for chunk in bodies.chunks(JORF_BATCH_SIZE) {
            written += repo
                .fill_empty_legal_text_bodies(chunk)
                .await
                .map_err(|e| anyhow!("fill_empty_legal_text_bodies: {e}"))?;
        }
        Ok(written)
    }
}

/// The text of an unnumbered article, `None` for a numbered or empty one.
fn unnumbered_text(art: &lj_extract::jorf::JorfArticle) -> Option<String> {
    if art.num.is_some() {
        return None;
    }
    art.texte
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Convertit une date ISO (`YYYY-MM-DD`) émise par le parser pur en
/// `NaiveDate`. Erreur franche si le format est invalide.
fn jorf_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date JORF invalide {iso:?}: {e}"))
}

/// Message de la passe textes : la ligne, son statut véhicule et ses liens
/// texte-niveau, écrits pour les fiches nouvelles.
struct JorfTexteMsg {
    row: lj_store::repository::LegalTextRow,
    vehicle: bool,
    liens: Vec<lj_extract::jorf::JorfLien>,
}

/// Convertit un [`lj_extract::jorf::JorfTexte`] en [`JorfTexteMsg`], identité
/// native du XML (`jurisdiction='FR'`, nature du JO). `title` = le libellé
/// descriptif `TITREFULL` si présent (pour que `title_key` matche les
/// citations), à défaut `TITRE` court. `date_texte`/`date_publi` natifs du JO.
///
/// `vehicles` = cibles d'une arête `publie` entrante : le décret de publication
/// d'un instrument migré sur fiche propre passe par l'upsert intégral.
fn jorf_texte_msg(
    mut t: lj_extract::jorf::JorfTexte,
    vehicles: &std::collections::HashSet<String>,
) -> JorfTexteMsg {
    let liens = std::mem::take(&mut t.liens);
    let vehicle = vehicles.contains(&t.jorftext);
    // Cascade d'identité (capturée avant les moves de `t`) : filet pour les actes
    // numérotés sans ELI/NOR.
    let instrument_key = lj_extract::instrument_key::instrument_key(
        &t.nature,
        t.date_texte.as_deref(),
        t.num.as_deref(),
    );
    let eli = t.eli;
    let nor = t.nor;
    let title = t.titre_full.unwrap_or(t.titre);
    let title_key = lj_extract::extract::normalize_instrument(&title);
    let date_texte = t
        .date_texte
        .as_deref()
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let date_publi = t
        .date_publi
        .as_deref()
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let row = lj_store::repository::LegalTextRow {
        text_uid: t.jorftext,
        jurisdiction: "FR".to_string(),
        title,
        title_key,
        nature: t.nature,
        // `last_modified` (incrémentalité) = publication JO ; date_texte en repli.
        last_modified: date_publi.or(date_texte),
        date_texte,
        date_publi,
        eli,
        nor,
        instrument_key,
        body: None,
        status: None,
    };
    JorfTexteMsg {
        row,
        vehicle,
        liens,
    }
}

/// Convertit les [`lj_extract::jorf::JorfLien`] d'un texte en lignes
/// [`LegalLinkRow`] : `verb` par le mapping `typelien` partagé, direction lue du
/// `sens` DILA (`source` = la cible agit sur l'owner), cible en clé pendante
/// résolue au read-time.
fn jorf_link_rows(
    liens: Vec<lj_extract::jorf::JorfLien>,
) -> Result<Vec<lj_store::repository::LegalLinkRow>> {
    liens
        .into_iter()
        .map(|l| {
            let target_date = match l.date_signa {
                Some(d) => Some(jorf_date(&d)?),
                None => None,
            };
            let target_kind = match l.target_id.as_deref() {
                Some(id) if id.contains("ARTI") => "article",
                Some(id) if id.contains("SCTA") => "section",
                _ => "texte",
            };
            let target_num_key = l
                .num
                .as_deref()
                .map(|n| {
                    lj_norm::article_key::article_key(&lj_extract::extract::normalize_article(n))
                })
                .filter(|k| !k.is_empty());
            Ok(lj_store::repository::LegalLinkRow {
                verb: lj_extract::legi::lien_verb(&l.typelien),
                typelien: l.typelien,
                direction: if l.sens == "source" {
                    "incoming"
                } else {
                    "outgoing"
                }
                .to_string(),
                target_kind: target_kind.to_string(),
                target_uid: l.target_id,
                target_text_uid: (!l.cid.is_empty()).then_some(l.cid),
                target_num: l.num,
                target_num_key,
                target_nature: l.nature,
                target_label: l.libelle.unwrap_or_default(),
                target_date,
                target_nor: None,
            })
        })
        .collect()
}

/// Convertit un [`lj_extract::jorf::JorfArticle`] en [`LegalArticleRow`].
/// `None` si l'article n'a pas de numéro (annonce/acte autonome — non citable,
/// hors référentiel versionné). `source='jorf'` (diffuseur DILA) : la ligne est
/// une **publication** au Journal officiel, pas une version consolidée — datée
/// par sa parution, sans fin de validité. L'identité de la ligne porte `source`,
/// donc elle coexiste avec la version LEGI du même article.
fn jorf_article_row(
    art: lj_extract::jorf::JorfArticle,
    content_checksum: u64,
) -> Result<Option<lj_store::repository::LegalArticleRow>> {
    let (Some(num), Some(num_key)) = (art.num, art.num_key) else {
        return Ok(None);
    };
    let source = "jorf".to_string();
    let date_debut = match art.date_debut {
        Some(d) => Some(jorf_date(&d)?),
        None => None,
    };
    let date_fin = match art.date_fin {
        Some(d) => Some(jorf_date(&d)?),
        None => None,
    };
    Ok(Some(lj_store::repository::LegalArticleRow {
        text_uid: art.jorftext,
        num,
        num_key,
        // L'ordre de lecture intra-texte du JO n'est pas exposé ici → None.
        position: None,
        title_path: art.titre_text,
        status: art.etat,
        date_debut,
        date_fin,
        texte: art.texte,
        // Droit FR/traités au JO : texte officiel, monolingue côté base.
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: art.nota,
        content_checksum,
        source,
        source_uid: art.jorfarti,
        source_url: None,
        // Bulk DILA (ingest manuel/périodique) : fraîcheur = la date d'ingest,
        // stable par ligne.
        source_asof: Some(chrono::Utc::now().date_naive()),
        source_upstream_url: None,
    }))
}

/// Créneau `(texte, numéro)` occupé par le dispositif propre d'un JORFTEXT, tel
/// que sa `STRUCT` le déclare.
fn dispositif_slot(text_uid: &str, num_key: &str) -> String {
    format!("{text_uid}/{num_key}")
}

/// Clé d'identité d'un article dans son texte. Le dispositif garde son numéro
/// nu ; l'article de section qui rejoue un numéro du dispositif porte sa
/// section dans sa clé (`1` sous « Annexe » donne `annexe-1`), sans quoi les
/// deux séries se disputent la PK `(text_uid, num_key, date_debut)` et la
/// seconde n'entre jamais.
fn article_num_key(
    text_uid: &str,
    num_key: &str,
    title_path: Option<&str>,
    dispositif: &HashSet<String>,
) -> String {
    match title_path.filter(|p| !p.is_empty()) {
        Some(path) if dispositif.contains(&dispositif_slot(text_uid, num_key)) => {
            format!("{}-{num_key}", lj_norm::article_key::article_key(path))
        }
        _ => num_key.to_string(),
    }
}

/// Ingère un stock ou un incrément JORF (`tar.gz` bulk DILA) en deux passes sur
/// l'archive :
/// 1. **textes** → `referential_texts` (tag `treaty`/`jorf`) ;
/// 2. **articles** → `referential_articles`, taggés selon les `cid` de traités
///    (lus en base entre les passes).
///
/// `paths` are local `Freemium_jorf_global_*.tar.gz` (stock) and/or `JORF_*.tar.gz`
/// increments, read in order, the finishers running **once** at the end of the
/// batch. Streaming bounded by channel as in [`super::legi`]: the only memory that
/// grows with the archive is the index of the `dispositif` slots.
pub async fn ingest_jorf(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Finisseurs pleine-table : au-delà du `statement_timeout` du pool.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    for path in paths {
        ingest_jorf_archive(&repo, path).await?;
    }
    // Chemin manuel (dont stock cold-start) : rôles sur la pleine table.
    finish_jorf(&repo, None).await
}

/// Texts, articles, then issues and publications. Returns the `text_uid`s the
/// role classification is scoped to; the caller runs [`finish_jorf`].
async fn ingest_jorf_archive(
    repo: &DecisionRepository<'_>,
    path: &Path,
) -> Result<HashSet<String>> {
    // Passe 1 : textes.
    let mut texts = ingest_jorf_texts(repo, path).await?;
    let dispositif = std::mem::take(&mut texts.dispositif);

    // Routes véhicule → fiche instrument, par arête `publie`. Un véhicule publié
    // par deux instruments rendrait le routage ambigu : erreur franche.
    let mut publie_routes: HashMap<String, String> = HashMap::new();
    for (vehicle, instrument) in repo
        .publie_vehicle_instrument_pairs()
        .await
        .map_err(|e| anyhow!("publie_vehicle_instrument_pairs: {e}"))?
    {
        if let Some(prev) = publie_routes.insert(vehicle.clone(), instrument.clone()) {
            if prev != instrument {
                return Err(anyhow!(
                    "véhicule {vehicle} cible d'arêtes publie de deux instruments \
                     ({prev}, {instrument}) — routage articles ambigu"
                ));
            }
        }
    }

    // Passe 2 : articles (routés par arête publie) + suppressions `.dat`.
    let routes = publie_routes.len();
    let (upserted, skipped, no_num, errors, deleted, bodies) =
        ingest_jorf_articles(repo, path, publie_routes, dispositif).await?;
    super::jorf_publication::ingest_publications(repo, path).await?;

    tracing::info!(
        source = %path.display(),
        vehicles_upserted = texts.vehicles,
        texts_created = texts.created,
        texts_existing = texts.existing,
        text_links = texts.links,
        publie_routes = routes,
        articles_upserted = upserted,
        articles_skipped = skipped,
        articles_no_num = no_num,
        articles_deleted = deleted,
        bodies,
        errors,
        "ingest_jorf_archive"
    );
    Ok(texts.touched)
}

/// Finisseurs post-archives, dans l'ordre des dépendances : slugs, puis rôles
/// (scopés aux uids touchés, ou fond complet si `None`), puis dénorm des
/// articles en dernier (`legal_article.searchable` dérive du slug ET du rôle).
async fn finish_jorf(
    repo: &DecisionRepository<'_>,
    touched: Option<&HashSet<String>>,
) -> Result<()> {
    // Slugs des textes nouveaux (unique écrivain de la colonne).
    let slugged = super::slugs::assign_text_slugs(repo).await?;

    // Rôles dérivés : les fiches fraîches du JO (nominations, habilitations,
    // véhicules) sortent de la recherche par défaut dès leur entrée.
    let (individuel, habilitation, vehicule, modificatif) = match touched {
        Some(touched) => {
            let uids: Vec<String> = touched.iter().cloned().collect();
            repo.classify_text_roles_scoped(&uids)
                .await
                .map_err(|e| anyhow!("classify_text_roles_scoped: {e}"))?
        }
        None => repo
            .backfill_text_roles()
            .await
            .map_err(|e| anyhow!("backfill_text_roles: {e}"))?,
    };

    let touched_uids: Option<Vec<String>> = touched.map(|t| t.iter().cloned().collect());
    let dated = repo
        .date_instruments_from_vehicles(touched_uids.as_deref())
        .await
        .map_err(|e| anyhow!("date_instruments_from_vehicles: {e}"))?;
    let treaties =
        super::treaty_articles::cut_treaty_articles(repo, touched_uids.as_deref()).await?;
    // Titre du code dénormalisé → titre formé `search_title`.
    let retitled = repo
        .refresh_article_denorm(None)
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
    // Ordre de lecture des textes touchés : l'écriture n'a fait qu'invalider les
    // rangs, ce finisseur les recalcule.
    let (ranked_texts, ranked_rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    tracing::info!(ranked_texts, ranked_rows, "reading ranks");

    tracing::info!(
        retitled,
        slugged,
        individuel,
        habilitation,
        vehicule,
        modificatif,
        instruments_dated = dated.len(),
        ?treaties,
        "finish_jorf"
    );
    Ok(())
}

/// Sync du fond JORF complet, auto-switch cold ↔ warm (même mécanique que les
/// autres fonds DILA) : télécharge le stock global au 1er run puis les
/// incréments postérieurs au watermark, ingère les archives fraîchement
/// téléchargées, puis passe les finisseurs globaux **une fois**.
pub async fn sync_jorf() -> Result<()> {
    let settings = Settings::from_env()?;
    let cache_dir = settings.cache_dir();
    let mut batch = tokio::task::spawn_blocking(move || {
        lj_sources::dila::sync_dila(&cache_dir, lj_sources::dila::DilaFond::Jorf)
    })
    .await
    .map_err(|e| anyhow!("sync_dila join (jorf): {e}"))?
    .map_err(|e| anyhow!("sync_dila (jorf): {e}"))?;
    if batch.archives.is_empty() {
        tracing::info!("sync_jorf: nothing new (<= watermark)");
        return Ok(());
    }
    tracing::info!(
        downloaded = batch.archives.len(),
        "sync_jorf: ingesting the fresh archives"
    );
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Finisseurs pleine-table : au-delà du `statement_timeout` du pool.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut touched = HashSet::new();
    for archive in batch.archives.clone() {
        touched.extend(ingest_jorf_archive(&repo, &archive.path).await?);
        batch
            .commit(&archive)
            .map_err(|e| anyhow!("filigrane jorf {}: {e}", archive.watermark))?;
    }
    finish_jorf(&repo, Some(&touched)).await
}

/// Rereads the unnumbered articles of every cached JORF archive, stock then
/// increments, and writes the body of the texts with neither body nor article.
pub async fn backfill_jorf_bodies() -> Result<()> {
    let settings = Settings::from_env()?;
    let paths = lj_sources::dila::cached_dila_tarballs(
        &settings.cache_dir(),
        lj_sources::dila::DilaFond::Jorf,
    )?;
    let unnumbered = tokio::task::spawn_blocking(move || -> Result<UnnumberedBodies> {
        let mut unnumbered = UnnumberedBodies::default();
        for path in &paths {
            lj_sources::tar_reader::for_each_member(path, |name, raw| {
                if !matches!(classify_jorf_member(&name), JorfMember::Article) {
                    return Ok(());
                }
                match lj_extract::jorf::parse_jorf_article(&raw) {
                    Ok(art) => {
                        if let Some(texte) = unnumbered_text(&art) {
                            unnumbered.push(art.jorftext, art.jorfarti, texte);
                        }
                    }
                    Err(e) => tracing::error!(member = %name, error = %e, "jorf article: parse failed"),
                }
                Ok(())
            })?;
            tracing::info!(archive = %path.display(), texts = unnumbered.0.len(), "jorf bodies: archive read");
        }
        Ok(unnumbered)
    })
    .await
    .map_err(|e| anyhow!("jorf bodies join: {e}"))??;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let candidates = unnumbered.0.len();
    let written = unnumbered.write(&DecisionRepository::new(&conn)).await?;
    tracing::warn!(candidates, written, "jorf bodies: empty texts filled");
    Ok(())
}

/// Compteurs de la passe textes : traités upsertés, fiches JO créées, fiches JO
/// déjà portées (LEGI/TNC ou run antérieur — jamais réécrites), liens écrits.
/// `touched` = les `text_uid` écrits par la passe (fiches créées/upsertées +
/// cibles des arêtes `publie`) — le scope de la classification des rôles.
#[derive(Default)]
struct JorfTextCounts {
    vehicles: usize,
    created: usize,
    existing: usize,
    links: u64,
    touched: HashSet<String>,
    /// Créneaux `(texte, numéro)` du dispositif propre, lus des `texte/struct`
    /// de l'archive : ce que la passe articles oppose aux séries de section.
    dispositif: HashSet<String>,
}

/// Passe 1 — textes JORF → `legal_text`, **fond complet** : les véhicules
/// (cibles `publie`) gardent l'upsert intégral (re-titrage post-restitution) ;
/// tout le reste du JO entre en fiche métadonnées (sans corps ni articles) via
/// [`insert_legal_text_if_absent`] — un JORFTEXT déjà porté par LEGI n'est
/// jamais écrasé. Les fiches créées écrivent leurs liens texte-niveau
/// (`legal_link`, graphe de généalogie).
///
/// [`insert_legal_text_if_absent`]: DecisionRepository::insert_legal_text_if_absent
async fn ingest_jorf_texts(repo: &DecisionRepository<'_>, path: &Path) -> Result<JorfTextCounts> {
    // Véhicules de publication connus : leur identité native (DECRET/FR) prime
    // sur l'override traité.
    let vehicles: Arc<std::collections::HashSet<String>> = Arc::new(
        repo.published_vehicle_uids()
            .await
            .map_err(|e| anyhow!("published_vehicle_uids: {e}"))?
            .into_iter()
            .collect(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<JorfTexteMsg>(JORF_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<HashSet<String>> {
        let mut dispositif = HashSet::new();
        lj_sources::tar_reader::for_each_member(
            &tar_path,
            |name, raw| match classify_jorf_member(&name) {
                JorfMember::Struct => match lj_extract::jorf::parse_jorf_struct(&raw) {
                    Ok(s) => {
                        dispositif.extend(s.articles().filter_map(|(_, num)| {
                            let key = lj_norm::article_key::identity_key(num?);
                            Some(dispositif_slot(&s.jorftext, &key))
                        }));
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "jorf struct: parse failed");
                        Ok(())
                    }
                },
                JorfMember::Texte => match lj_extract::jorf::parse_jorf_texte(&raw) {
                    Ok(t) => tx
                        .blocking_send(jorf_texte_msg(t, &vehicles))
                        .map_err(|_| anyhow!("canal JORF textes fermé")),
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "jorf texte: parse failed");
                        Ok(())
                    }
                },
                _ => Ok(()),
            },
        )?;
        Ok(dispositif)
    });

    let mut batch: Vec<JorfTexteMsg> = Vec::new();
    let mut counts = JorfTextCounts::default();
    while let Some(msg) = rx.recv().await {
        batch.push(msg);
        if batch.len() >= JORF_BATCH_SIZE {
            flush_jorf_texts(repo, std::mem::take(&mut batch), &mut counts).await?;
        }
    }
    counts.dispositif = reader
        .await
        .map_err(|e| anyhow!("tâche lecture JORF textes {}: {e}", path.display()))??;
    if !batch.is_empty() {
        flush_jorf_texts(repo, batch, &mut counts).await?;
    }
    Ok(counts)
}

/// Pass 2: JORF articles into `referential_articles`, and the bodies of the
/// empty texts. Returns `(upserted, skipped, no_num, errors, deleted, bodies)`.
async fn ingest_jorf_articles(
    repo: &DecisionRepository<'_>,
    path: &Path,
    publie_routes: HashMap<String, String>,
    dispositif: HashSet<String>,
) -> Result<(usize, usize, usize, usize, usize, u64)> {
    enum ArtMsg {
        Row(Box<lj_store::repository::LegalArticleRow>),
        NoNum(Option<(String, String, String)>),
        Err,
        Dat(Vec<String>),
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ArtMsg>(JORF_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            // Suppressions JORF : `liste_suppression_jorf*.dat` (un id par ligne),
            // appliquées après les upserts de la passe.
            if name.to_lowercase().ends_with(".dat") {
                let paths: Vec<String> = String::from_utf8_lossy(&raw)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                if !paths.is_empty() {
                    tx.blocking_send(ArtMsg::Dat(paths))
                        .map_err(|_| anyhow!("canal JORF articles fermé"))?;
                }
                return Ok(());
            }
            if !matches!(classify_jorf_member(&name), JorfMember::Article) {
                return Ok(());
            }
            let msg = match lj_extract::jorf::parse_jorf_article(&raw) {
                Ok(art) if art.num.is_none() => {
                    ArtMsg::NoNum(unnumbered_text(&art).map(|t| (art.jorftext, art.jorfarti, t)))
                }
                Ok(art) => {
                    let route = publie_routes.get(&art.jorftext).cloned();
                    let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                    match jorf_article_row(art, checksum) {
                        Ok(Some(mut row)) => {
                            row.num_key = article_num_key(
                                &row.text_uid,
                                &row.num_key,
                                row.title_path.as_deref(),
                                &dispositif,
                            );
                            // Le routage véhicule → instrument : sur un cid
                            // véhicule, le texte de l'instrument publié en annexe
                            // (num non numérique) dépose sur la fiche instrument.
                            // Le dispositif du véhicule (nums numériques) reste
                            // sur la sienne.
                            if let Some(instrument) = route {
                                if !row.num_key.bytes().all(|b| b.is_ascii_digit()) {
                                    row.text_uid = instrument;
                                }
                            }
                            ArtMsg::Row(Box::new(row))
                        }
                        Ok(None) => ArtMsg::NoNum(None),
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "jorf article: row invalide");
                            ArtMsg::Err
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "jorf article: parse failed");
                    ArtMsg::Err
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal JORF articles fermé"))
        })
    });

    let mut batch: Vec<lj_store::repository::LegalArticleRow> = Vec::new();
    let mut suppress: Vec<String> = Vec::new();
    let (mut upserted, mut skipped, mut no_num, mut errors) = (0usize, 0usize, 0usize, 0usize);
    let mut unnumbered = UnnumberedBodies::default();
    while let Some(msg) = rx.recv().await {
        match msg {
            ArtMsg::Row(row) => {
                batch.push(*row);
                if batch.len() >= JORF_BATCH_SIZE {
                    flush_jorf_articles(
                        repo,
                        std::mem::take(&mut batch),
                        &mut upserted,
                        &mut skipped,
                    )
                    .await?;
                }
            }
            ArtMsg::NoNum(art) => {
                no_num += 1;
                if let Some((jorftext, jorfarti, texte)) = art {
                    unnumbered.push(jorftext, jorfarti, texte);
                }
            }
            ArtMsg::Err => errors += 1,
            ArtMsg::Dat(paths) => suppress.extend(paths),
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture JORF articles {}: {e}", path.display()))??;
    if !batch.is_empty() {
        flush_jorf_articles(repo, batch, &mut upserted, &mut skipped).await?;
    }
    let deleted = if suppress.is_empty() {
        0
    } else {
        repo.delete_legal_articles_by_paths("jorf", &suppress)
            .await
            .map_err(|e| anyhow!("delete_legal_articles_by_paths: {e}"))? as usize
    };
    let bodies = unnumbered.write(repo).await?;
    Ok((upserted, skipped, no_num, errors, deleted, bodies))
}

/// Écrit un batch de textes JORF : traité ou véhicule de publication → upsert
/// intégral (JORF est la seule source des décrets absents de LEGI) ; sinon
/// insertion si absent (règle d'autorité LEGI) + liens texte-niveau des fiches
/// créées (batchés par [`DecisionRepository::replace_legal_links`], rejouables
/// — remplacement par propriétaire).
async fn flush_jorf_texts(
    repo: &DecisionRepository<'_>,
    texts: Vec<JorfTexteMsg>,
    counts: &mut JorfTextCounts,
) -> Result<()> {
    let mut links: Vec<(
        lj_store::repository::LegalLinkOwner,
        Vec<lj_store::repository::LegalLinkRow>,
    )> = Vec::new();
    for msg in texts {
        if msg.vehicle {
            repo.upsert_legal_text(&msg.row)
                .await
                .map_err(|e| anyhow!("upsert_legal_text {}: {e}", msg.row.text_uid))?;
            counts.vehicles += 1;
            counts.touched.insert(msg.row.text_uid.clone());
        } else if repo
            .insert_legal_text_if_absent(&msg.row)
            .await
            .map_err(|e| anyhow!("insert_legal_text_if_absent {}: {e}", msg.row.text_uid))?
        {
            counts.created += 1;
            counts.touched.insert(msg.row.text_uid.clone());
            if !msg.liens.is_empty() {
                let owner = lj_store::repository::LegalLinkOwner {
                    text_uid: msg.row.text_uid.clone(),
                    num_key: String::new(),
                    date_debut: msg.row.date_texte,
                };
                let rows = jorf_link_rows(msg.liens)?;
                // Cibles des arêtes `publie` entrantes : candidates au rôle
                // `vehicule`, dans le scope.
                for r in &rows {
                    if r.verb == "publie" && r.direction == "incoming" {
                        if let Some(target) = &r.target_text_uid {
                            counts.touched.insert(target.clone());
                        }
                    }
                }
                links.push((owner, rows));
            }
        } else {
            counts.existing += 1;
        }
    }
    if !links.is_empty() {
        counts.links += repo
            .replace_legal_links(&links)
            .await
            .map_err(|e| anyhow!("replace_legal_links (jorf): {e}"))?;
    }
    Ok(())
}

/// Upsert d'un batch d'articles JORF ; cumule modifiés/skippés.
async fn flush_jorf_articles(
    repo: &DecisionRepository<'_>,
    articles: Vec<lj_store::repository::LegalArticleRow>,
    upserted: &mut usize,
    skipped: &mut usize,
) -> Result<()> {
    for art in articles {
        if repo
            .upsert_legal_article(&art, lj_store::repository::Fraicheur::Autoritaire)
            .await
            .map_err(|e| anyhow!("upsert_legal_article {}: {e}", art.source_uid))?
        {
            *upserted += 1;
        } else {
            *skipped += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_deux_series_d_un_meme_jorftext_gardent_deux_identites() {
        // Arrêté du 31 juillet 2026 (JORFTEXT000054750162, permis de lithium
        // « Bassin de Limagne ») : six articles au dispositif, et une annexe
        // « cahier des charges » qui renumérote de 1 à 3.
        let texte = "JORFTEXT000054750162";
        let dispositif: HashSet<String> = ["1", "2", "3", "4", "5", "6"]
            .iter()
            .map(|n| dispositif_slot(texte, n))
            .collect();

        assert_eq!(article_num_key(texte, "1", None, &dispositif), "1");
        assert_eq!(
            article_num_key(texte, "1", Some("Annexe"), &dispositif),
            "annexe-1"
        );
        assert_eq!(
            article_num_key(texte, "3", Some("Annexe"), &dispositif),
            "annexe-3"
        );
    }

    #[test]
    fn l_article_d_une_section_qui_ne_rejoue_rien_garde_son_numero_nu() {
        // Le dispositif d'un texte à chapitres court d'un chapitre à l'autre :
        // sa numérotation est celle du texte, et les clés publiques du fond ne
        // bougent pas.
        let texte = "JORFTEXT000054483646";
        let dispositif: HashSet<String> = ["1", "2"]
            .iter()
            .map(|n| dispositif_slot(texte, n))
            .collect();
        assert_eq!(
            article_num_key(
                texte,
                "3",
                Some("Chapitre II : Dispositions finales"),
                &dispositif
            ),
            "3"
        );
    }

    #[test]
    fn deux_sections_qui_rejouent_le_meme_numero_prennent_deux_cles() {
        // Décret 2003-462 (JORFTEXT000000412528) : trois articles « 1 », un au
        // dispositif et un dans chacune de deux parties du code annexé.
        let texte = "JORFTEXT000000412528";
        let dispositif: HashSet<String> = [dispositif_slot(texte, "1")].into_iter().collect();
        assert_eq!(
            article_num_key(
                texte,
                "1",
                Some("Chapitre V : Dispositions pénales"),
                &dispositif
            ),
            "chapitre-v-dispositions-penales-1"
        );
        assert_eq!(
            article_num_key(texte, "1", Some("Livre VII : Prévention"), &dispositif),
            "livre-vii-prevention-1"
        );
    }
}
