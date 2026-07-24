//! KALI (conventions collectives nationales, bulk DILA, ADR 0120) : ingest
//! tarball stock/incrément + sync incrémental. Calque de [`super::legi`] (même
//! mécanique tar→parse→upsert, même canal borné, même watermark `sync_dila`) ;
//! seuls le parser pur (`lj_extract::kali`) et l'ancrage `text_uid = KALICONT`
//! (la convention, pas l'avenant) diffèrent.

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

/// Convertit une date ISO (`YYYY-MM-DD`) en `NaiveDate`. Erreur franche (#12).
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

/// [`LegalArticleRow`] depuis un [`lj_extract::kali::KaliArticle`] : `text_uid` =
/// KALICONT (la convention), identité `(text_uid, num_key, date_debut)`. Provenance
/// `source='kali'`, `source_uid` = KALIARTI.
fn kali_article_row(
    art: lj_extract::kali::KaliArticle,
    content_checksum: u64,
) -> Result<lj_store::repository::LegalArticleRow> {
    let date_fin = match art.date_fin {
        Some(d) => Some(kali_date(&d)?),
        None => None,
    };
    Ok(lj_store::repository::LegalArticleRow {
        text_uid: art.kalicont,
        num: art.num,
        num_key: art.num_key,
        // Le tar KALI n'expose pas l'ordre de lecture → repli `num_key` (ADR 0112 §9).
        position: None,
        title_path: art.titre_text,
        status: art.etat,
        date_debut: Some(kali_date(&art.date_debut)?),
        date_fin,
        texte: art.texte,
        // Convention collective FR officielle, monolingue (ADR 0116).
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum,
        source: "kali".to_string(),
        source_uid: art.kaliarti,
        source_url: None,
        // Source *live* autoritaire : fraîcheur dérivée de `ingest_freshness` (ADR 0129).
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
        // Identité d'une convention = son KALICONT (pas la cascade ELI/NOR ADR 0115).
        eli: None,
        nor: None,
        instrument_key: None,
        body: None,
        status: None,
    })
}

/// Ingère un stock ou un incrément KALI (`tar.gz` bulk DILA, ADR 0120). Bootstrap
/// manuel d'un tarball depuis le disque (dev/reprise) sans toucher au watermark.
pub async fn ingest_kali(path: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Le refresh `code_title` final est un UPDATE-jointure pleine table (scan de tout
    // `legal_article`) : il dépasse le statement_timeout par défaut → on le lève pour
    // la session (l'ingest est un batch hors-ligne, pas une requête servie).
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    ingest_kali_tarball(&repo, path).await?;
    // Dénormalise le titre de la convention sur ses articles (search_title, ADR 0114).
    let retitled = repo
        .refresh_article_denorm()
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(retitled, slugged, "ingest_kali refresh code_title");
    Ok(())
}

/// Ingère un unique `tar.gz` KALI via `repo` (boucle membres → parse → upsert +
/// suppressions `.dat`). Renvoie `(upserted, skipped, errors)`. Idempotent par
/// `content_checksum` (#7). Calque de `ingest_legi_tarball`.
async fn ingest_kali_tarball(
    repo: &DecisionRepository<'_>,
    path: &Path,
) -> Result<(usize, usize, usize)> {
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

    // Lecture+parsing (gzip+tar+XML CPU sync) en thread bloquant, BORNÉE par un
    // canal : RAM ~constante (le stock a ~480 k membres). Le DB reste côté async.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<KaliMsg>(KALI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if name.to_lowercase().ends_with(".dat") {
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
                    // Article sans numéro (non citable) → hors référentiel ;
                    // le corps des TI les consomme à part (ADR 0223).
                    Ok(art) if art.num_key.is_empty() => return Ok(()),
                    Ok(mut art) => {
                        let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                        let liens = std::mem::take(&mut art.liens);
                        let built = kali_article_row(art, checksum).and_then(|row| {
                            let owner = lj_store::repository::LegalLinkOwner {
                                text_uid: row.text_uid.clone(),
                                num_key: row.num_key.clone(),
                                date_debut: row.date_debut,
                            };
                            Ok((row, super::legi::legal_link_rows(&owner, liens)?))
                        });
                        match built {
                            Ok(pair) => KaliMsg::Article(Box::new(pair)),
                            Err(e) => {
                                tracing::error!(member = %name, error = %e, "kali article: row invalide");
                                KaliMsg::ParseErr
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "kali article: parse échec");
                        KaliMsg::ParseErr
                    }
                },
                KaliMember::Conteneur => match lj_extract::kali::parse_kali_conteneur(&raw) {
                    Ok(cont) => match kali_conteneur_row(cont) {
                        Ok(row) => KaliMsg::Conteneur(Box::new(row)),
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "kali conteneur: row invalide");
                            KaliMsg::ParseErr
                        }
                    },
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "kali conteneur: parse échec");
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
                articles.push(*row);
                if articles.len() >= KALI_BATCH_SIZE {
                    let batch = std::mem::take(&mut articles);
                    flush_kali_articles(repo, batch, &mut upserted, &mut skipped).await?;
                }
            }
            KaliMsg::Conteneur(row) => {
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
        flush_kali_articles(repo, articles, &mut upserted, &mut skipped).await?;
    }
    if !conteneurs.is_empty() {
        flush_kali_conteneurs(repo, conteneurs).await?;
    }

    // Suppressions après les upserts (un incrément ajoute ET retire, #7).
    let mut deleted: u64 = 0;
    for paths in dat_paths {
        deleted += repo
            .delete_legal_articles_by_paths("kali", &paths)
            .await
            .map_err(|e| anyhow!("delete_legal_articles_by_paths: {e}"))?;
    }

    // Le refresh `code_title` (dénorm. titre convention → articles, search_title
    // ADR 0114) est fait UNE fois en fin de run par l'appelant (`ingest_kali` /
    // `sync_kali`), pas par tarball : c'est un UPDATE-jointure pleine table, à ne pas
    // répéter sur chaque incrément d'un rattrapage.
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

/// Upsert d'un batch d'articles KALI (idempotent #7) ; cumule modifiés/skippés.
/// Les arêtes `legal_link` des articles réellement écrits sont remplacées dans
/// la foulée (ADR 0174, même règle que LEGI).
async fn flush_kali_articles(
    repo: &DecisionRepository<'_>,
    articles: Vec<(
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    )>,
    upserted: &mut usize,
    skipped: &mut usize,
) -> Result<()> {
    let mut link_items: Vec<(
        lj_store::repository::LegalLinkOwner,
        Vec<lj_store::repository::LegalLinkRow>,
    )> = Vec::new();
    for (art, links) in articles {
        if repo
            .upsert_legal_article(&art)
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

/// Upsert d'un batch de conteneurs KALI vers `legal_text` (ON CONFLICT `text_uid`).
async fn flush_kali_conteneurs(
    repo: &DecisionRepository<'_>,
    conteneurs: Vec<lj_store::repository::LegalTextRow>,
) -> Result<()> {
    for cont in conteneurs {
        repo.upsert_legal_text(&cont)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", cont.text_uid))?;
    }
    Ok(())
}

/// Sync incrémental KALI (ADR 0120/0093) : `sync_dila` télécharge stock (cold) puis
/// incréments postérieurs au watermark, chacun ingéré par [`ingest_kali_tarball`].
pub async fn sync_kali() -> Result<()> {
    let settings = Settings::from_env()?;
    let downloaded =
        lj_sources::dila::sync_dila(&settings.cache_dir(), lj_sources::dila::DilaFond::Kali)
            .map_err(|e| anyhow!("sync_dila kali: {e}"))?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Le refresh `code_title` final est un UPDATE-jointure pleine table (scan de tout
    // `legal_article`) : il dépasse le statement_timeout par défaut → on le lève pour
    // la session (l'ingest est un batch hors-ligne, pas une requête servie).
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    for path in &downloaded {
        let (u, s, e) = ingest_kali_tarball(&repo, path).await?;
        upserted += u;
        skipped += s;
        errors += e;
    }

    // Refresh `code_title` une seule fois, après tous les tarballs du run.
    let retitled = if downloaded.is_empty() {
        0
    } else {
        repo.refresh_article_denorm()
            .await
            .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?
    };

    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;

    tracing::info!(
        increments = downloaded.len(),
        upserted,
        skipped,
        errors,
        retitled,
        slugged,
        "sync_kali"
    );
    Ok(())
}
