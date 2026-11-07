//! KALI (conventions collectives nationales, bulk DILA) : ingest tarball
//! stock/incrément + sync incrémental. Calque de [`super::legi`] (même
//! mécanique tar→parse→upsert, même canal borné, même watermark `sync_dila`) ;
//! seuls le parser pur (`lj_extract::kali`) et l'ancrage `text_uid = KALICONT`
//! (la convention, pas l'avenant) diffèrent.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Taille de batch des upserts KALI (articles puis conteneurs).
const KALI_BATCH_SIZE: usize = 512;

/// Sentinelles de date DILA → « pas de date » (cf. `lj_extract::legi`).
const DATE_SENTINELS: &[&str] = &["2999-01-01", "2222-02-22"];

/// Classe d'un membre XML du fond KALI, déduite de son chemin.
///
/// Les `ARTICLE` (`…/article/…/KALIARTI*.xml`) et les `CONTENEUR`
/// (`…/conteneur/…/KALICONT*.xml`) sont ingérés ; les `KALITEXT` (`texte/struct/`,
/// l'avenant) et `KALISCTA` (`section_ta/`, l'arbre des sections) sont hors
/// périmètre : le `text_uid` s'ancre sur le conteneur et le fil d'Ariane vient du
/// `TM` porté par chaque ARTICLE (comme LEGI).
pub(crate) enum KaliMember {
    Article,
    Conteneur,
    Ignore,
}

pub(crate) fn classify_kali_member(name: &str) -> KaliMember {
    let lower = name.to_lowercase();
    let stem = name.rsplit('/').next().unwrap_or(name);
    if lower.contains("/article/") && stem.starts_with("KALIARTI") {
        return KaliMember::Article;
    }
    if lower.contains("/conteneur/") && stem.starts_with("KALICONT") {
        return KaliMember::Conteneur;
    }
    KaliMember::Ignore
}

/// Convertit une date ISO (`YYYY-MM-DD`) en `NaiveDate`. Erreur franche.
fn kali_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date KALI invalide {iso:?}: {e}"))
}

/// Date ISO optionnelle → `NaiveDate`, sentinelle/absente → `None`.
fn kali_date_opt(iso: Option<&str>) -> Result<Option<chrono::NaiveDate>> {
    match iso {
        Some(d) if !DATE_SENTINELS.contains(&d) && !d.is_empty() => Ok(Some(kali_date(d)?)),
        _ => Ok(None),
    }
}

/// Clé d'identité d'un article KALI : nue sur la **pièce de tête** du recueil,
/// qualifiée par sa pièce ailleurs. `tete` vaut `None` tant que le recueil n'a
/// pas transité, et sa numérotation reste alors nue de bout en bout.
fn kali_num_key(art: &lj_extract::kali::KaliArticle, tete: Option<&str>) -> String {
    match (tete, art.kalitext.as_deref()) {
        (Some(tete), Some(piece)) if piece != tete => {
            lj_norm::article_key::piece_key(piece, &art.num_key)
        }
        _ => art.num_key.clone(),
    }
}

/// Fil d'Ariane d'un article de recueil : sa pièce en tête, puis les sections.
/// Nu lui aussi tant que le recueil n'a pas transité.
fn kali_title_path(art: &lj_extract::kali::KaliArticle, tete: Option<&str>) -> Option<String> {
    let piece = tete.and(art.kalitext_titre.as_deref());
    match (piece, art.titre_text.as_deref()) {
        (Some(piece), Some(tm)) => Some(format!("{piece} > {tm}")),
        (Some(piece), None) => Some(piece.to_string()),
        (None, tm) => tm.map(str::to_string),
    }
}

/// [`LegalArticleRow`] depuis un [`lj_extract::kali::KaliArticle`] : `text_uid` =
/// KALICONT (la convention), identité `(text_uid, num_key, date_debut)`, provenance
/// `source_uid` = KALIARTI. `tete` = pièce de tête du conteneur.
pub(super) fn kali_article_row(
    art: lj_extract::kali::KaliArticle,
    content_checksum: u64,
    tete: Option<&str>,
) -> Result<lj_store::repository::LegalArticleRow> {
    let date_debut = Some(kali_date(&art.date_debut)?);
    let date_fin = match art.date_fin.as_deref() {
        Some(d) => Some(kali_date(d)?),
        None => None,
    };
    let num_key = kali_num_key(&art, tete);
    let title_path = kali_title_path(&art, tete);
    Ok(lj_store::repository::LegalArticleRow {
        text_uid: art.kalicont,
        num: art.num,
        num_key,
        // Le tar KALI n'expose pas l'ordre de lecture → repli `num_key`.
        position: None,
        title_path,
        status: art.etat,
        date_debut,
        date_fin,
        texte: art.texte,
        // Convention collective FR officielle, monolingue.
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum,
        source: "kali".to_string(),
        source_uid: art.kaliarti,
        source_url: None,
        // Source *live* autoritaire : fraîcheur dérivée de `ingest_freshness`.
        source_asof: None,
        source_upstream_url: None,
    })
}

/// [`LegalTextRow`] `jurisdiction='FR'` depuis un [`lj_extract::kali::KaliConteneur`].
/// `text_uid` = KALICONT (identité), `title_key` =
/// `normalize_instrument(titre)` (même vocabulaire que les clés de capture du linker).
fn kali_conteneur_row(
    cont: lj_extract::kali::KaliConteneur,
) -> Result<lj_store::repository::LegalTextRow> {
    let date_publi = kali_date_opt(cont.date_publi.as_deref())?;
    let title_key = lj_extract::extract::normalize_instrument(&cont.titre);
    Ok(lj_store::repository::LegalTextRow {
        text_uid: cont.kalicont,
        jurisdiction: "FR".to_string(),
        title: cont.titre,
        title_key,
        nature: cont.nature,
        last_modified: date_publi,
        date_texte: None,
        date_publi,
        // Identité d'une convention = son KALICONT, pas la cascade ELI/NOR.
        eli: None,
        nor: None,
        instrument_key: None,
        body: None,
        status: None,
    })
}

/// Ingère un stock ou un incrément KALI (`tar.gz` bulk DILA). Bootstrap manuel
/// d'un tarball depuis le disque (dev/reprise) sans toucher au watermark.
pub async fn ingest_kali(
    paths: &[std::path::PathBuf],
    textes: &[String],
    stock_ancien: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // The end-of-pass steps are full-table passes, and an ingest is an offline batch
    // rather than a served request.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let fraicheur = if stock_ancien {
        lj_store::repository::Fraicheur::Rejeu
    } else {
        lj_store::repository::Fraicheur::Autoritaire
    };
    let mut touches: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (rang, path) in paths.iter().enumerate() {
        let (u, s, e) = ingest_kali_tarball(&repo, path, textes, fraicheur, &mut touches).await?;
        tracing::info!(
            rang = rang + 1,
            total = paths.len(),
            archive = %path.display(),
            upserted = u, skipped = s, errors = e,
            "ingest_kali archive"
        );
    }
    // Slugs first: the scoped pass below copies them onto the articles.
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    let portee: Vec<String> = touches.into_iter().collect();
    // Dénormalise le titre de la convention sur ses articles (`search_title`).
    let retitled = repo
        .refresh_article_denorm(Some(&portee))
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
    // Ordre de lecture des textes touchés : l'écriture n'a fait qu'invalider les
    // rangs, ce finisseur les recalcule.
    let (ranked_texts, ranked_rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    tracing::info!(ranked_texts, ranked_rows, "reading ranks");
    tracing::info!(retitled, slugged, "ingest_kali refresh code_title");
    Ok(())
}

/// Ingère un unique `tar.gz` KALI via `repo` (boucle membres → parse → upsert +
/// suppressions `.dat`). Renvoie `(upserted, skipped, errors)`. Idempotent par
/// `content_checksum`. Calque de `ingest_legi_tarball`, à une passe près.
async fn ingest_kali_tarball(
    repo: &DecisionRepository<'_>,
    path: &Path,
    textes: &[String],
    fraicheur: lj_store::repository::Fraicheur,
    touches: &mut std::collections::HashSet<String>,
) -> Result<(usize, usize, usize)> {
    // Pièces de tête relues en base et non dans le tarball : un incrément livre
    // des articles dont le conteneur n'est pas du voyage, et c'est la
    // requalification, pas l'ingestion, qui décide qu'un recueil a transité.
    let tetes: HashMap<String, String> = repo
        .base_pieces()
        .await
        .map_err(|e| anyhow!("base_pieces: {e}"))?
        .into_iter()
        .collect();

    enum KaliMsg {
        Article(
            Box<(
                lj_store::repository::LegalArticleRow,
                Vec<lj_store::repository::LegalLinkRow>,
            )>,
        ),
        Conteneur(Box<lj_store::repository::LegalTextRow>),
        Dat(Vec<String>),
        ParseErr,
    }

    // Reading and parsing (gzip, tar, XML: sync CPU) in a blocking thread, BOUNDED
    // by a channel, so RAM stays flat whatever the stock. The DB stays async.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<KaliMsg>(KALI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let filtre: std::collections::HashSet<String> = textes.iter().cloned().collect();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            // A KALI member does not name its container in its path, unlike LEGI where
            // the text cid is a directory of the article: the link sits inside the XML,
            // so a named pass filters after the parse and never on the path.
            if name.to_lowercase().ends_with(".dat") {
                // A named pass never deletes, so it drops the suppression lists.
                if !filtre.is_empty() {
                    return Ok(());
                }
                let paths: Vec<String> = String::from_utf8_lossy(&raw)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                if !paths.is_empty() {
                    tx.blocking_send(KaliMsg::Dat(paths))
                        .map_err(|_| anyhow!("canal KALI fermé (consumer arrêté)"))?;
                }
                return Ok(());
            }
            let msg = match classify_kali_member(&name) {
                KaliMember::Article => match lj_extract::kali::parse_kali_article(&raw) {
                    // A numberless article cannot be cited, so it stays out
                    // of the referential: the TI bodies consume it apart.
                    Ok(art) if art.num_key.is_empty() => return Ok(()),
                    Ok(art) if !filtre.is_empty() && !filtre.contains(&art.kalicont) => {
                        return Ok(())
                    }
                    Ok(mut art) => {
                        let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                        let liens = std::mem::take(&mut art.liens);
                        let tete = tetes.get(&art.kalicont).map(String::as_str);
                        let built = kali_article_row(art, checksum, tete)
                            .and_then(|row| Ok((row, super::legi::legal_link_rows(liens)?)));
                        match built {
                            Ok(pair) => KaliMsg::Article(Box::new(pair)),
                            Err(e) => {
                                tracing::error!(member = %name, error = %e, "kali article: row invalide");
                                KaliMsg::ParseErr
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "kali article: parse failed");
                        KaliMsg::ParseErr
                    }
                },
                KaliMember::Conteneur => match lj_extract::kali::parse_kali_conteneur(&raw) {
                    Ok(cont) if !filtre.is_empty() && !filtre.contains(&cont.kalicont) => {
                        return Ok(())
                    }
                    Ok(cont) => match kali_conteneur_row(cont) {
                        Ok(row) => KaliMsg::Conteneur(Box::new(row)),
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "kali conteneur: row invalide");
                            KaliMsg::ParseErr
                        }
                    },
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "kali container: parse failed");
                        KaliMsg::ParseErr
                    }
                },
                KaliMember::Ignore => return Ok(()),
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal KALI fermé (consumer arrêté)"))
        })
    });

    let mut articles: Vec<(
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    )> = Vec::new();
    let mut conteneurs: Vec<lj_store::repository::LegalTextRow> = Vec::new();
    let mut dat_paths: Vec<Vec<String>> = Vec::new();
    let (mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize);

    while let Some(msg) = rx.recv().await {
        match msg {
            KaliMsg::Article(row) => {
                touches.insert(row.0.text_uid.clone());
                articles.push(*row);
                if articles.len() >= KALI_BATCH_SIZE {
                    let batch = std::mem::take(&mut articles);
                    flush_kali_articles(repo, batch, &mut upserted, &mut skipped, fraicheur)
                        .await?;
                }
            }
            KaliMsg::Conteneur(row) => {
                touches.insert(row.text_uid.clone());
                conteneurs.push(*row);
                if conteneurs.len() >= KALI_BATCH_SIZE {
                    let batch = std::mem::take(&mut conteneurs);
                    flush_kali_conteneurs(repo, batch).await?;
                }
            }
            KaliMsg::Dat(paths) => dat_paths.push(paths),
            KaliMsg::ParseErr => errors += 1,
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture KALI {}: {e}", path.display()))??;

    if !articles.is_empty() {
        flush_kali_articles(repo, articles, &mut upserted, &mut skipped, fraicheur).await?;
    }
    if !conteneurs.is_empty() {
        flush_kali_conteneurs(repo, conteneurs).await?;
    }

    // Suppressions après les upserts (un incrément ajoute ET retire).
    let mut deleted: u64 = 0;
    for paths in dat_paths {
        deleted += repo
            .delete_legal_articles_by_paths("kali", &paths)
            .await
            .map_err(|e| anyhow!("delete_legal_articles_by_paths: {e}"))?;
    }

    // Le refresh `code_title` (dénorm. titre convention → articles, `search_title`)
    // est fait UNE fois en fin de run par l'appelant (`ingest_kali` / `sync_kali`),
    // pas par tarball : c'est un UPDATE-jointure pleine table, à ne pas répéter sur
    // chaque incrément d'un rattrapage.
    tracing::info!(
        source = %path.display(),
        upserted,
        skipped,
        deleted,
        errors,
        "ingest_kali"
    );
    Ok((upserted, skipped, errors))
}

/// Upsert d'un batch d'articles KALI (idempotent) ; cumule modifiés/skippés.
/// Les arêtes `legal_link` des articles réellement écrits sont remplacées dans
/// la foulée, même règle que LEGI.
async fn flush_kali_articles(
    repo: &DecisionRepository<'_>,
    articles: Vec<(
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    )>,
    upserted: &mut usize,
    skipped: &mut usize,
    fraicheur: lj_store::repository::Fraicheur,
) -> Result<()> {
    let mut link_items: Vec<(
        lj_store::repository::LegalLinkOwner,
        Vec<lj_store::repository::LegalLinkRow>,
    )> = Vec::new();
    for (art, links) in articles {
        if repo
            .upsert_legal_article(&art, fraicheur)
            .await
            .map_err(|e| anyhow!("upsert_legal_article {}: {e}", art.source_uid))?
        {
            *upserted += 1;
            link_items.push((
                lj_store::repository::LegalLinkOwner {
                    text_uid: art.text_uid,
                    num_key: art.num_key,
                    date_debut: art.date_debut,
                },
                links,
            ));
        } else {
            *skipped += 1;
        }
    }
    repo.replace_legal_links(&link_items)
        .await
        .map_err(|e| anyhow!("replace_legal_links (kali): {e}"))?;
    Ok(())
}

/// Upsert d'un batch de conteneurs KALI (= conventions) vers `legal_text`.
async fn flush_kali_conteneurs(
    repo: &DecisionRepository<'_>,
    conteneurs: Vec<lj_store::repository::LegalTextRow>,
) -> Result<()> {
    for row in conteneurs {
        repo.upsert_legal_text(&row)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", row.text_uid))?;
    }
    Ok(())
}

/// Sync incrémental KALI : `sync_dila` télécharge stock (cold) puis incréments
/// postérieurs au watermark, chacun ingéré par [`ingest_kali_tarball`] et suivi
/// de son filigrane — une passe interrompue ne saute rien.
pub async fn sync_kali() -> Result<()> {
    let settings = Settings::from_env()?;
    let mut batch =
        lj_sources::dila::sync_dila(&settings.cache_dir(), lj_sources::dila::DilaFond::Kali)
            .map_err(|e| anyhow!("sync_dila kali: {e}"))?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Le refresh `code_title` final est un UPDATE-jointure pleine table (scan de tout
    // `legal_article`) : il dépasse le statement_timeout par défaut → on le lève pour
    // la session (l'ingest est un batch hors-ligne, pas une requête servie).
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    let mut touches: std::collections::HashSet<String> = std::collections::HashSet::new();
    for archive in batch.archives.clone() {
        let (u, s, e) = ingest_kali_tarball(
            &repo,
            &archive.path,
            &[],
            lj_store::repository::Fraicheur::Autoritaire,
            &mut touches,
        )
        .await?;
        upserted += u;
        skipped += s;
        errors += e;
        batch
            .commit(&archive)
            .map_err(|e| anyhow!("filigrane kali {}: {e}", archive.watermark))?;
    }

    // Après le sync, qui vient de poser les conteneurs du jour : un recueil neuf
    // transite la nuit de son arrivée. Le budget de watchdog de la chaîne est
    // plein, donc la requalification suit ici plutôt que dans une étape à elle.
    let requalifies = requalifier_recueils(&repo, &settings, REQUALIF_LOT).await?;

    // Slugs first: the scoped pass below copies them onto the articles.
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    let portee: Vec<String> = touches.into_iter().collect();
    // A requalification rewrites texts the archive loop never listed, so it
    // widens the pass back to the corpus.
    let cible = if requalifies > 0 {
        None
    } else {
        Some(portee.as_slice())
    };
    let retitled = repo
        .refresh_article_denorm(cible)
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
        increments = batch.archives.len(),
        upserted,
        skipped,
        errors,
        requalifies,
        retitled,
        slugged,
        "sync_kali"
    );
    Ok(())
}

/// Collections requalified per pass. Bounds the RAM: the batch's articles are held
/// in memory for one walk of the cache.
const REQUALIF_LOT: i64 = 200;

/// Les recueils du fond dans `legal_text`. Les pièces n'y sont pas des textes,
/// donc le préfixe suffit à les nommer tous.
const KALI_RECUEIL_PREFIX: &str = "KALICONT%";

/// Ce que le cache DILA dit aujourd'hui d'un recueil : sa pièce de tête et ses
/// articles vivants, le dernier tarball qui parle d'un article ayant le dernier
/// mot.
#[derive(Default)]
struct EtatRecueil {
    tete: Option<String>,
    articles: HashMap<String, (lj_extract::kali::KaliArticle, u64)>,
}

/// État des recueils d'un lot, reconstruit à chaque passage depuis le cache : ce
/// qui rend une reprise après coupure propre, aucun état de parcours ne
/// survivant au processus.
#[derive(Default)]
struct LotRecueils(HashMap<String, EtatRecueil>);

impl LotRecueils {
    fn voit_article(&mut self, art: lj_extract::kali::KaliArticle, checksum: u64) {
        self.0
            .entry(art.kalicont.clone())
            .or_default()
            .articles
            .insert(art.kaliarti.clone(), (art, checksum));
    }

    fn voit_tete(&mut self, kalicont: String, tete: String) {
        self.0.entry(kalicont).or_default().tete = Some(tete);
    }

    /// Retire les articles qu'un `.dat` d'incrément déclare supprimés : le rejeu
    /// additif ne ressuscite pas ce que la source a retiré. Le nom du fichier
    /// porte le `source_uid`, comme pour la suppression en base.
    fn oublie(&mut self, chemins: &[String]) {
        for chemin in chemins {
            let stem = chemin.rsplit('/').next().unwrap_or(chemin);
            let uid = stem.strip_suffix(".xml").unwrap_or(stem);
            for etat in self.0.values_mut() {
                etat.articles.remove(uid);
            }
        }
    }
}

/// Carte de requalification et lignes à écrire d'un recueil, tirées du **même**
/// état de cache : tout article vu entre dans les deux. C'est ce qui donne son
/// sens à `TransitRecueil::non_couvertes`, une ligne en base hors carte étant
/// une ligne que le cache ne décrit plus.
type PlanRecueil = (
    Vec<(String, String, Option<String>)>,
    Vec<(
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    )>,
);

fn plan_recueil(
    articles: HashMap<String, (lj_extract::kali::KaliArticle, u64)>,
    tete: &str,
) -> Result<PlanRecueil> {
    let (mut carte, mut lignes) = (Vec::new(), Vec::new());
    for (mut art, checksum) in articles.into_values() {
        let prefixe = prefixe_de_piece(art.kalitext.as_deref(), tete);
        carte.push((
            art.kaliarti.clone(),
            prefixe,
            kali_title_path(&art, Some(tete)),
        ));
        let liens = std::mem::take(&mut art.liens);
        lignes.push((
            kali_article_row(art, checksum, Some(tete))?,
            super::legi::legal_link_rows(liens)?,
        ));
    }
    Ok((carte, lignes))
}

/// Préfixe que la requalification pose devant la `num_key` d'un article : vide
/// sur la pièce de tête, sinon la pièce pliée. Le concaténer à un numéro nu doit
/// rendre exactement la clé que [`kali_num_key`] écrirait.
fn prefixe_de_piece(piece: Option<&str>, tete: &str) -> String {
    match piece {
        Some(piece) if piece != tete => format!("{}-", lj_norm::article_key::article_key(piece)),
        _ => String::new(),
    }
}

/// Passage manuel de requalification, pour accélérer la transition sans
/// attendre les nuits : le sync quotidien en fait un lot de [`REQUALIF_LOT`].
pub async fn requalifier_kali(lot: i64) -> Result<()> {
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
    if requalifier_recueils(&repo, &settings, lot).await? > 0 {
        repo.refresh_article_denorm(None)
            .await
            .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
        repo.refresh_reading_ranks()
            .await
            .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    }
    Ok(())
}

/// Fait passer un lot de recueils KALI à la clé qualifiée et rend le nombre de
/// recueils passés. Le curseur est `legal_text.base_piece_uid` : nulle = recueil
/// à requalifier, et une transaction par recueil ne le laisse jamais à cheval
/// sur les deux clés. Rien à requalifier ⇒ retour immédiat, sans lire le cache.
async fn requalifier_recueils(
    repo: &DecisionRepository<'_>,
    settings: &Settings,
    lot: i64,
) -> Result<usize> {
    let cibles = repo
        .recueils_a_requalifier(KALI_RECUEIL_PREFIX, lot)
        .await
        .map_err(|e| anyhow!("recueils_a_requalifier: {e}"))?;
    if cibles.is_empty() {
        return Ok(0);
    }
    let tarballs = lj_sources::dila::cached_dila_tarballs(
        &settings.cache_dir(),
        lj_sources::dila::DilaFond::Kali,
    )
    .map_err(|e| anyhow!("cached_dila_tarballs kali: {e}"))?;
    let mut etats = lire_recueils(&tarballs, &cibles).await?;

    let (mut recueils, mut requalifies, mut porteuses, mut ecrits, mut incomplets) =
        (0usize, 0u64, 0u64, 0u64, 0usize);
    for uid in &cibles {
        let etat = etats.0.remove(uid).unwrap_or_default();
        let Some(tete) = etat.tete else {
            incomplets += 1;
            continue;
        };
        let (carte, articles) = plan_recueil(etat.articles, &tete)?;
        let transit = repo
            .transiter_recueil(uid, &tete, &carte, &articles)
            .await
            .map_err(|e| anyhow!("transiter_recueil {uid}: {e}"))?;
        // Le passage s'annule de lui-même quand une ligne en base échappe à la
        // carte : marquée, elle resterait nue sous un recueil qualifié.
        if transit.non_couvertes > 0 {
            incomplets += 1;
            tracing::warn!(
                recueil = %uid,
                non_couvertes = transit.non_couvertes,
                "requalification cancelled: some articles in the database are outside the cache"
            );
            continue;
        }
        recueils += 1;
        requalifies += transit.requalifies;
        porteuses += transit.porteuses;
        ecrits += transit.ecrits;
    }

    let restants = repo
        .recueils_a_requalifier(KALI_RECUEIL_PREFIX, 1)
        .await
        .map_err(|e| anyhow!("recueils_a_requalifier: {e}"))?
        .len();
    tracing::info!(
        recueils,
        requalifies,
        porteuses,
        ecrits,
        incomplets,
        fini = restants == 0,
        "requalifier_kali"
    );
    Ok(recueils)
}

/// Relit le cache DILA dans l'ordre de rejeu et rend l'état courant des recueils
/// demandés. Le `.dat` d'un incrément retire l'article du lot : le rejeu additif
/// ne doit pas ressusciter ce que la source a supprimé.
async fn lire_recueils(tarballs: &[std::path::PathBuf], cibles: &[String]) -> Result<LotRecueils> {
    enum LotMsg {
        Article(Box<(lj_extract::kali::KaliArticle, u64)>),
        Tete(String, String),
        Dat(Vec<String>),
    }

    let lot: std::collections::HashSet<String> = cibles.iter().cloned().collect();
    let mut etats = LotRecueils::default();
    for path in tarballs {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<LotMsg>(KALI_BATCH_SIZE * 4);
        let tar_path = path.clone();
        let lot_lecture = lot.clone();
        let reader = tokio::task::spawn_blocking(move || -> Result<()> {
            lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
                let msg = if name.to_lowercase().ends_with(".dat") {
                    let paths: Vec<String> = String::from_utf8_lossy(&raw)
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect();
                    if paths.is_empty() {
                        return Ok(());
                    }
                    LotMsg::Dat(paths)
                } else {
                    match classify_kali_member(&name) {
                        KaliMember::Article => match lj_extract::kali::parse_kali_article(&raw) {
                            Ok(art)
                                if !art.num_key.is_empty()
                                    && lot_lecture.contains(&art.kalicont) =>
                            {
                                let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                                LotMsg::Article(Box::new((art, checksum)))
                            }
                            _ => return Ok(()),
                        },
                        KaliMember::Conteneur => {
                            match lj_extract::kali::parse_kali_conteneur(&raw) {
                                Ok(cont)
                                    if lot_lecture.contains(&cont.kalicont)
                                        && !cont.textes.is_empty() =>
                                {
                                    LotMsg::Tete(cont.kalicont, cont.textes[0].clone())
                                }
                                _ => return Ok(()),
                            }
                        }
                        KaliMember::Ignore => return Ok(()),
                    }
                };
                tx.blocking_send(msg)
                    .map_err(|_| anyhow!("canal KALI fermé (consumer arrêté)"))
            })
        });

        while let Some(msg) = rx.recv().await {
            match msg {
                LotMsg::Article(art) => {
                    let (art, checksum) = *art;
                    etats.voit_article(art, checksum);
                }
                LotMsg::Tete(cont, tete) => etats.voit_tete(cont, tete),
                LotMsg::Dat(paths) => etats.oublie(&paths),
            }
        }
        reader
            .await
            .map_err(|e| anyhow!("tâche lecture KALI {}: {e}", path.display()))??;
    }
    Ok(etats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(piece: Option<&str>, num_key: &str) -> lj_extract::kali::KaliArticle {
        lj_extract::kali::KaliArticle {
            kaliarti: "KALIARTI000005831775".into(),
            kalicont: "KALICONT000005635552".into(),
            kalitext: piece.map(str::to_string),
            kalitext_titre: Some("Annexe I - Ouvriers".into()),
            num: num_key.into(),
            num_key: num_key.into(),
            titre_text: Some("Champ d'application".into()),
            etat: "VIGUEUR".into(),
            date_debut: "1961-07-10".into(),
            date_fin: None,
            texte: None,
            liens: Vec::new(),
        }
    }

    const TETE: &str = "KALITEXT000005675425";
    const ANNEXE: &str = "KALITEXT000005675711";

    #[test]
    fn la_piece_de_tete_garde_son_numero_nu_les_autres_le_qualifient() {
        assert_eq!(kali_num_key(&art(Some(TETE), "2"), Some(TETE)), "2");
        assert_eq!(
            kali_num_key(&art(Some(ANNEXE), "2"), Some(TETE)),
            "kalitext000005675711-2"
        );
        // Deux pièces attachées portant le même numéro cessent de se disputer le créneau.
        assert_ne!(
            kali_num_key(&art(Some(ANNEXE), "2"), Some(TETE)),
            kali_num_key(&art(Some("KALITEXT000005675733"), "2"), Some(TETE))
        );
    }

    #[test]
    fn la_quotidienne_ne_met_pas_un_recueil_a_cheval() {
        // Recueil pas encore requalifié (`base_piece_uid` nulle) : l'incrément du
        // jour écrit la clé nue et le fil nu, comme la base les porte.
        assert_eq!(kali_num_key(&art(Some(ANNEXE), "2"), None), "2");
        assert_eq!(
            kali_title_path(&art(Some(ANNEXE), "2"), None).as_deref(),
            Some("Champ d'application")
        );
        // Article ancré sur le conteneur, sans pièce : rien à qualifier.
        assert_eq!(kali_num_key(&art(None, "2"), Some(TETE)), "2");
    }

    #[test]
    fn la_requalification_vise_la_cle_que_l_ingest_ecrirait() {
        for piece in [Some(TETE), Some(ANNEXE), None] {
            for num in ["1", "2", "11 bis", "L. 3-1"] {
                let art = art(piece, &lj_norm::article_key::identity_key(num));
                let prefixe = prefixe_de_piece(art.kalitext.as_deref(), TETE);
                assert_eq!(
                    format!("{prefixe}{}", art.num_key),
                    kali_num_key(&art, Some(TETE))
                );
            }
        }
    }

    #[test]
    fn le_fil_d_ariane_commence_par_la_piece() {
        assert_eq!(
            kali_title_path(&art(Some(ANNEXE), "2"), Some(TETE)).as_deref(),
            Some("Annexe I - Ouvriers > Champ d'application")
        );
        let mut sans_tm = art(Some(ANNEXE), "2");
        sans_tm.titre_text = None;
        assert_eq!(
            kali_title_path(&sans_tm, Some(TETE)).as_deref(),
            Some("Annexe I - Ouvriers")
        );
        let mut sans_piece = art(None, "2");
        sans_piece.kalitext_titre = None;
        assert_eq!(
            kali_title_path(&sans_piece, Some(TETE)).as_deref(),
            Some("Champ d'application")
        );
    }

    #[test]
    fn le_plan_couvre_exactement_les_articles_que_le_cache_decrit() {
        let mut vus = HashMap::new();
        for (uid, piece) in [("KALIARTI1", Some(TETE)), ("KALIARTI2", Some(ANNEXE))] {
            let mut a = art(piece, "2");
            a.kaliarti = uid.into();
            vus.insert(uid.to_string(), (a, 7u64));
        }
        let (carte, lignes) = plan_recueil(vus, TETE).expect("plan");
        let mut couverts: Vec<&str> = carte.iter().map(|c| c.0.as_str()).collect();
        couverts.sort_unstable();
        assert_eq!(couverts, ["KALIARTI1", "KALIARTI2"]);
        // Les deux listes sortent du même état : une ligne en base hors carte est
        // donc une ligne que le cache ne décrit plus, et rien d'autre.
        let mut ecrits: Vec<&str> = lignes.iter().map(|l| l.0.source_uid.as_str()).collect();
        ecrits.sort_unstable();
        assert_eq!(couverts, ecrits);

        // Recueil vide des deux côtés : rien à couvrir, donc rien qui échappe, et
        // la transaction pose sa pièce de tête sans annuler.
        let (carte, lignes) = plan_recueil(HashMap::new(), TETE).expect("plan vide");
        assert!(carte.is_empty() && lignes.is_empty());
    }

    #[test]
    fn le_lot_relu_garde_la_derniere_version_et_oublie_le_supprime() {
        let mut lot = LotRecueils::default();
        lot.voit_tete("KALICONT000005635552".into(), TETE.into());
        let mut stock = art(Some(ANNEXE), "2");
        stock.texte = Some("version du stock".into());
        lot.voit_article(stock, 1);
        let mut increment = art(Some(ANNEXE), "2");
        increment.texte = Some("version de l'incrément".into());
        lot.voit_article(increment, 2);

        let etat = &lot.0["KALICONT000005635552"];
        assert_eq!(etat.tete.as_deref(), Some(TETE));
        assert_eq!(etat.articles.len(), 1);
        assert_eq!(
            etat.articles["KALIARTI000005831775"].0.texte.as_deref(),
            Some("version de l'incrément")
        );

        lot.oublie(&["kali/global/article/KALI/ARTI/KALIARTI000005831775.xml".into()]);
        assert!(lot.0["KALICONT000005635552"].articles.is_empty());
    }
}
