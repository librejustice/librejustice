//! LEGI (référentiel versionné) : ingest tarball stock/incrément, sync
//! incrémental, résolution citation→article.

use std::path::Path;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Taille de batch des upserts LEGI (articles puis codes).
const LEGI_BATCH_SIZE: usize = 512;

/// Classe d'un membre XML du fond LEGI, déduite de son chemin.
///
/// Les `ARTICLE` (`…/article/…/LEGIARTI*.xml`), les `TEXTE_VERSION`
/// (`…/norm/version/…/LEGITEXT*.xml`), les `TEXTELR` (`…/norm/struct/…`) et
/// les `SECTION_TA` (`…/section_ta/…`) sont ingérés ; l'arbre `eli/` est
/// hors-périmètre.
pub(super) enum LegiMember {
    Article,
    Texte,
    SectionTa,
    Struct,
    Ignore,
}

/// Classe un membre par son chemin tar. Insensible à la casse de l'extension ;
/// la discrimination se fait sur le nom de fichier (`LEGIARTI`/`LEGITEXT`/
/// `LEGISCTA`) sous le bon dossier.
pub(super) fn classify_legi_member(name: &str) -> LegiMember {
    let lower = name.to_lowercase();
    let stem = name.rsplit('/').next().unwrap_or(name);
    if lower.contains("/eli/") {
        return LegiMember::Ignore;
    }
    if lower.contains("section_ta") && stem.starts_with("LEGISCTA") {
        return LegiMember::SectionTa;
    }
    if lower.contains("/texte/struct/") && stem.starts_with("LEGITEXT") {
        return LegiMember::Struct;
    }
    if lower.contains("/article/") && stem.starts_with("LEGIARTI") {
        return LegiMember::Article;
    }
    if lower.contains("/texte/version/") && stem.starts_with("LEGITEXT") {
        return LegiMember::Texte;
    }
    LegiMember::Ignore
}

/// Convertit une date ISO (`YYYY-MM-DD`) émise par le parser pur en `NaiveDate`
/// au bord store. Erreur franche si le format est invalide.
fn legi_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date LEGI invalide {iso:?}: {e}"))
}

/// Convertit une [`lj_extract::legi::LegiArticle`] (dates ISO `String`) en
/// [`LegalArticleRow`] : `text_uid` = LEGITEXT parent, identité de version
/// `(text_uid, num_key, date_debut)`. Provenance LEGI : `source_uid` = LEGIARTI,
/// le CID d'où l'URL Légifrance se dérive. `num_key` (déjà `normalize_article`
/// côté parser pur) et `title_path` (`titre_text`) tels quels.
fn legi_article_row(
    art: lj_extract::legi::LegiArticle,
    content_checksum: u64,
) -> Result<lj_store::repository::LegalArticleRow> {
    let date_debut = Some(legi_date(&art.date_debut)?);
    let date_fin = match art.date_fin {
        Some(d) => Some(legi_date(&d)?),
        None => None,
    };
    Ok(lj_store::repository::LegalArticleRow {
        text_uid: art.legitext,
        num: art.num,
        num_key: art.num_key,
        // Le tar LEGI n'expose pas l'ordre de lecture du code : `position` non
        // renseignée, lecture « à la suite » en repli sur `num_key`.
        position: None,
        title_path: art.titre_text,
        status: art.etat,
        date_debut,
        date_fin,
        texte: art.texte,
        // Droit FR officiel, monolingue.
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: art.nota,
        content_checksum,
        source: "legifrance".to_string(),
        source_uid: art.legiarti,
        source_url: None,
        // Source *live* autoritaire : fraîcheur dérivée de `ingest_freshness`, pas
        // stockée par ligne.
        source_asof: None,
        source_upstream_url: None,
    })
}

/// Convertit les [`lj_extract::legi::LegiLien`] d'un fichier en lignes
/// [`LegalLinkRow`] au bord store : dates parsées (sentinelles déjà absorbées
/// côté parser), direction décodée par le parser depuis `(typelien, sens)`
/// (`lj_extract::legi::lien_direction`), `target_num_key` = `normalize_article`
/// pour la résolution des cibles article sans ID. Partagé avec le pipeline KALI
/// (même bloc `<LIENS>`).
pub(crate) fn legal_link_rows(
    liens: Vec<lj_extract::legi::LegiLien>,
) -> Result<Vec<lj_store::repository::LegalLinkRow>> {
    liens
        .into_iter()
        .map(|l| {
            let target_date = match l.target_date {
                Some(d) => Some(legi_date(&d)?),
                None => None,
            };
            let target_num_key = l
                .target_num
                .as_deref()
                .map(|n| {
                    lj_norm::article_key::article_key(&lj_extract::extract::normalize_article(n))
                })
                .filter(|k| !k.is_empty());
            Ok(lj_store::repository::LegalLinkRow {
                typelien: l.typelien,
                verb: l.verb,
                direction: l.direction.to_string(),
                target_kind: l.target_kind.as_str().to_string(),
                target_uid: l.target_uid,
                target_text_uid: l.target_text_uid,
                target_num: l.target_num,
                target_num_key,
                target_nature: l.target_nature,
                target_label: l.target_label,
                target_date,
                target_nor: l.target_nor,
            })
        })
        .collect()
}

/// Convertit un [`lj_extract::legi::LegiToc`] (arbre structurel) en item
/// d'écriture `legal_toc_edge` au bord store : dates parsées, `seq` implicite
/// (ordre du `Vec`).
fn legi_toc_item(
    toc: lj_extract::legi::LegiToc,
) -> Result<(
    lj_store::repository::TocOwner,
    Vec<lj_store::repository::TocEdgeRow>,
)> {
    let edges = toc
        .edges
        .into_iter()
        .map(|e| {
            let date_debut = match e.date_debut {
                Some(d) => Some(legi_date(&d)?),
                None => None,
            };
            let date_fin = match e.date_fin {
                Some(d) => Some(legi_date(&d)?),
                None => None,
            };
            Ok(lj_store::repository::TocEdgeRow {
                child_kind: e.child_kind.as_str().to_string(),
                child_uid: e.child_uid,
                child_cid: e.child_cid,
                child_num_key: e.child_num_key,
                label: e.label,
                etat: e.etat,
                date_debut,
                date_fin,
                niv: e.niv,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((
        lj_store::repository::TocOwner {
            owner_uid: toc.owner_uid,
            text_uid: toc.text_uid,
        },
        edges,
    ))
}

/// Propriétaires `legal_toc_edge` visés par une liste de suppression `.dat` :
/// les stems `LEGISCTA` (versions de section) des chemins `section_ta` — un
/// fichier = un propriétaire, purge précise. Les racines (`texte/struct`, owner =
/// cid partagé entre versions) ne sont **pas** purgées ici : la suppression d'un
/// texte entier passe par `prune_orphan_toc_edges` (plus d'articles ⇒ plus
/// d'arbre).
fn toc_owners_from_paths(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| p.to_lowercase().contains("section_ta"))
        .map(|p| {
            p.rsplit('/')
                .next()
                .unwrap_or(p)
                .trim_end_matches(".xml")
                .to_string()
        })
        .filter(|stem| stem.starts_with("LEGISCTA"))
        .collect()
}

/// Convertit une [`lj_extract::legi::LegiCode`] (dates ISO `String`) en
/// [`LegalTextRow`] `jurisdiction='FR'`. `text_uid` = CID chronique (identité
/// partagée avec les articles et la TOC), `title` = TITREFULL descriptif quand
/// présent (le TITRE d'un TNC est nu), `title_key` = `normalize_instrument(titre)`.
/// La provenance vit sur les versions.
fn legi_code_row(code: lj_extract::legi::LegiCode) -> Result<lj_store::repository::LegalTextRow> {
    let last_modified = match code.derniere_modification {
        Some(d) => Some(legi_date(&d)?),
        None => None,
    };
    let date_texte = code.date_texte.as_deref().map(legi_date).transpose()?;
    let date_publi = code.date_publi.as_deref().map(legi_date).transpose()?;
    let title = code.titre_full.unwrap_or(code.titre);
    let title_key = lj_extract::extract::normalize_instrument(&title);
    Ok(lj_store::repository::LegalTextRow {
        text_uid: code.legitext,
        jurisdiction: "FR".to_string(),
        title,
        title_key,
        nature: code.nature,
        last_modified,
        date_texte,
        date_publi,
        // Identité d'un code = son slug, ni date ni numéro. Le NOR des actes datés
        // (arrêtés, décrets) sert de clé de résolution au linker.
        eli: None,
        nor: code.nor,
        instrument_key: None,
        status: None,
    })
}

/// A text version's blocks as pieces of the text, dated by the version.
fn legi_block_rows(
    code: &lj_extract::legi::LegiCode,
    content_checksum: u64,
) -> Result<Vec<lj_store::repository::LegalArticleRow>> {
    let date_debut = Some(legi_date(&code.version_debut)?);
    let date_fin = code.version_fin.as_deref().map(legi_date).transpose()?;
    Ok(code
        .blocks
        .iter()
        .map(|(key, texte)| lj_store::repository::LegalArticleRow {
            text_uid: code.legitext.clone(),
            num: String::new(),
            num_key: key.to_string(),
            position: lj_norm::article_order::block_position(key),
            title_path: None,
            status: code.version_etat.clone(),
            date_debut,
            date_fin,
            texte: Some(texte.clone()),
            texte_original: None,
            lang_original: None,
            translation: "officiel".to_string(),
            nota: None,
            content_checksum,
            source: "legifrance".to_string(),
            source_uid: code.version_id.clone(),
            source_url: None,
            source_asof: None,
            source_upstream_url: None,
        })
        .collect())
}

/// Ingère un stock ou un incrément LEGI (`tar.gz` bulk DILA).
///
/// Streaming des membres (`tar_reader::for_each_member`) : les `LEGIARTI*.xml`
/// sont parsés en [`lj_extract::legi::parse_legi_article`], les `LEGITEXT*.xml`
/// (sous `texte/version/`) en [`lj_extract::legi::parse_legi_texte`] ; le reste
/// est ignoré. `content_checksum` = xxh3-64 des octets bruts du membre ; l'upsert
/// article skippe les lignes au checksum inchangé. Les suppressions
/// (`liste_suppression_legi*.dat`) sont appliquées après les upserts via
/// `delete_legal_articles_by_paths` (canal séparé). `payload_format = 'dila-xml'`.
///
/// `path` = chemin local d'un `Freemium_legi_global_*.tar.gz` (stock) ou d'un
/// incrément `LEGI_*.tar.gz`.
pub async fn ingest_legi(
    paths: &[std::path::PathBuf],
    textes: &[String],
    stock_ancien: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // The post-loop steps are full-table passes, and an ingest is an offline batch
    // rather than a served request. `collapse_empty_legitext_doublons` has no timeout
    // of its own and precedes the finishers, so the default limit cancelled them.
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
    for (rang, path) in paths.iter().enumerate() {
        let (u, s, e) = ingest_legi_tarball(&repo, path, textes, fraicheur).await?;
        tracing::info!(
            rang = rang + 1,
            total = paths.len(),
            archive = %path.display(),
            upserted = u, skipped = s, errors = e,
            "ingest_legi archive"
        );
    }
    let collapsed = repo
        .collapse_empty_legitext_doublons()
        .await
        .map_err(|e| anyhow!("collapse_empty_legitext_doublons: {e}"))?;
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(collapsed, slugged, "ingest_legi collapse doublons");
    finir_passe_legi(&repo).await?;
    Ok(())
}

/// Ingère un unique `tar.gz` LEGI via `repo` (boucle membres → parse → upsert +
/// suppressions `.dat`). Partagé par [`ingest_legi`] et [`sync_legi`]. Renvoie
/// `(upserted, skipped, errors)`. Idempotent par `content_checksum`.
async fn ingest_legi_tarball(
    repo: &DecisionRepository<'_>,
    path: &Path,
    textes: &[String],
    fraicheur: lj_store::repository::Fraicheur,
) -> Result<(usize, usize, usize)> {
    // Messages remontés par le lecteur (membres parsés). Rows boxées (variantes de
    // tailles très inégales). Chaque row voyage avec les arêtes de son bloc <LIENS>.
    /// A text, its upcoming version dates, its links, and its blocks as pieces.
    type CodeMsg = (
        lj_store::repository::LegalTextRow,
        Vec<chrono::NaiveDate>,
        Vec<lj_store::repository::LegalLinkRow>,
        Vec<lj_store::repository::LegalArticleRow>,
    );
    enum LegiMsg {
        Article(
            Box<(
                lj_store::repository::LegalArticleRow,
                Vec<lj_store::repository::LegalLinkRow>,
            )>,
        ),
        Code(Box<CodeMsg>),
        Toc(
            Box<(
                lj_store::repository::TocOwner,
                Vec<lj_store::repository::TocEdgeRow>,
            )>,
        ),
        Dat(Vec<String>),
        Placeholder,
        ParseErr,
    }

    // Lecture et parsing (gzip + tar + XML, CPU sync) en thread bloquant, bornés par
    // un canal : la RAM reste constante sur un stock global de plus d'un million de
    // membres. Le DB reste côté async — le `repo` emprunte la connexion, non
    // déplaçable dans spawn_blocking. Backpressure : canal plein ⇒ le lecteur attend
    // sur blocking_send.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<LegiMsg>(LEGI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let filtre: std::collections::HashSet<String> = textes.iter().cloned().collect();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member_filtered(
            &tar_path,
            // A named pass drops the suppression lists with the rest: a repair never
            // deletes.
            |name| filtre.is_empty() || name.split('/').any(|seg| filtre.contains(seg)),
            |name, raw| {
                // Suppressions LEGI : `liste_suppression_legi*.dat` (chemins, un/ligne).
                if name.to_lowercase().ends_with(".dat") {
                    let paths: Vec<String> = String::from_utf8_lossy(&raw)
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect();
                    if !paths.is_empty() {
                        tx.blocking_send(LegiMsg::Dat(paths))
                            .map_err(|_| anyhow!("canal LEGI fermé (consumer arrêté)"))?;
                    }
                    return Ok(());
                }
                let msg = match classify_legi_member(&name) {
                    LegiMember::Article => match lj_extract::legi::parse_legi_article(&raw) {
                        Ok(art)
                            if art
                                .texte
                                .as_deref()
                                .is_some_and(lj_extract::legi::is_placeholder) =>
                        {
                            LegiMsg::Placeholder
                        }
                        Ok(mut art) => {
                            let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                            let liens = std::mem::take(&mut art.liens);
                            let built = legi_article_row(art, checksum)
                                .and_then(|row| Ok((row, legal_link_rows(liens)?)));
                            match built {
                                Ok(pair) => LegiMsg::Article(Box::new(pair)),
                                Err(e) => {
                                    tracing::error!(member = %name, error = %e, "legi article: row invalide");
                                    LegiMsg::ParseErr
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "legi article: parse failed");
                            LegiMsg::ParseErr
                        }
                    },
                    LegiMember::Texte => match lj_extract::legi::parse_legi_texte(&raw) {
                        Ok(mut code) => {
                            let liens = std::mem::take(&mut code.liens);
                            let upcoming = std::mem::take(&mut code.versions_a_venir);
                            let built = legi_block_rows(&code, xxhash_rust::xxh3::xxh3_64(&raw))
                                .and_then(|blocks| {
                                    Ok((
                                        legi_code_row(code)?,
                                        upcoming_dates(&upcoming)?,
                                        legal_link_rows(liens)?,
                                        blocks,
                                    ))
                                });
                            match built {
                                Ok(triple) => LegiMsg::Code(Box::new(triple)),
                                Err(e) => {
                                    tracing::error!(member = %name, error = %e, "legi texte: row invalide");
                                    LegiMsg::ParseErr
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "legi texte: parse failed");
                            LegiMsg::ParseErr
                        }
                    },
                    LegiMember::SectionTa => match lj_extract::legi::parse_legi_section_ta(&raw)
                        .map_err(anyhow::Error::from)
                        .and_then(legi_toc_item)
                    {
                        Ok(item) => LegiMsg::Toc(Box::new(item)),
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "legi section_ta: parse failed");
                            LegiMsg::ParseErr
                        }
                    },
                    LegiMember::Struct => match lj_extract::legi::parse_legi_textelr(&raw)
                        .map_err(anyhow::Error::from)
                        .and_then(legi_toc_item)
                    {
                        Ok(item) => LegiMsg::Toc(Box::new(item)),
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "legi textelr: parse failed");
                            LegiMsg::ParseErr
                        }
                    },
                    LegiMember::Ignore => return Ok(()),
                };
                tx.blocking_send(msg)
                    .map_err(|_| anyhow!("canal LEGI fermé (consumer arrêté)"))
            },
        )
    });

    type ArticleItem = (
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    );
    type CodeItem = (
        lj_store::repository::LegalTextRow,
        Vec<chrono::NaiveDate>,
        Vec<lj_store::repository::LegalLinkRow>,
    );
    type TocItem = (
        lj_store::repository::TocOwner,
        Vec<lj_store::repository::TocEdgeRow>,
    );
    let mut articles: Vec<ArticleItem> = Vec::new();
    let mut codes: Vec<CodeItem> = Vec::new();
    let mut tocs: Vec<TocItem> = Vec::new();
    let mut dat_paths: Vec<Vec<String>> = Vec::new();
    let (mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    let mut placeholders = 0usize;

    while let Some(msg) = rx.recv().await {
        match msg {
            LegiMsg::Article(row) => {
                articles.push(*row);
                if articles.len() >= LEGI_BATCH_SIZE {
                    let batch = std::mem::take(&mut articles);
                    flush_legi_articles(repo, batch, &mut upserted, &mut skipped, fraicheur)
                        .await?;
                }
            }
            LegiMsg::Code(row) => {
                let (text, upcoming, links, blocks) = *row;
                codes.push((text, upcoming, links));
                articles.extend(blocks.into_iter().map(|b| (b, Vec::new())));
                if codes.len() >= LEGI_BATCH_SIZE {
                    let batch = std::mem::take(&mut codes);
                    flush_legi_codes(repo, batch).await?;
                }
            }
            LegiMsg::Toc(item) => {
                tocs.push(*item);
                if tocs.len() >= LEGI_BATCH_SIZE {
                    let batch = std::mem::take(&mut tocs);
                    repo.replace_toc_edges(&batch)
                        .await
                        .map_err(|e| anyhow!("replace_toc_edges: {e}"))?;
                }
            }
            LegiMsg::Dat(paths) => dat_paths.push(paths),
            LegiMsg::Placeholder => placeholders += 1,
            LegiMsg::ParseErr => errors += 1,
        }
    }
    // Canal fermé = lecteur fini : remonter une éventuelle erreur de lecture/parse.
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture LEGI {}: {e}", path.display()))??;

    if !articles.is_empty() {
        flush_legi_articles(repo, articles, &mut upserted, &mut skipped, fraicheur).await?;
    }
    if !codes.is_empty() {
        flush_legi_codes(repo, codes).await?;
    }
    if !tocs.is_empty() {
        repo.replace_toc_edges(&tocs)
            .await
            .map_err(|e| anyhow!("replace_toc_edges: {e}"))?;
    }

    // Suppressions appliquées après les upserts : un incrément ajoute ET retire.
    let mut deleted: u64 = 0;
    for paths in dat_paths {
        deleted += repo
            .delete_legal_articles_by_paths("legifrance", &paths)
            .await
            .map_err(|e| anyhow!("delete_legal_articles_by_paths: {e}"))?;
        let toc_owners = toc_owners_from_paths(&paths);
        deleted += repo
            .delete_toc_edges_by_owners(&toc_owners)
            .await
            .map_err(|e| anyhow!("delete_toc_edges_by_owners: {e}"))?;
    }

    tracing::info!(
        source = %path.display(),
        upserted,
        skipped,
        deleted,
        placeholders,
        errors,
        "ingest_legi"
    );
    Ok((upserted, skipped, errors))
}

/// End-of-pass finisher, after the archive loop and never inside it: the reading
/// ranks reconcile the whole corpus.
async fn finir_passe_legi(repo: &DecisionRepository<'_>) -> Result<()> {
    let (ranked_texts, ranked_rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    tracing::info!(ranked_texts, ranked_rows, "reading ranks");
    Ok(())
}

/// Upsert d'un batch d'articles LEGI vers `referential_articles` ; cumule
/// modifiés/skippés. Les arêtes `legal_link` des articles réellement écrits sont
/// remplacées dans la foulée : checksum inchangé ⇒ liens inchangés.
async fn flush_legi_articles(
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
        .map_err(|e| anyhow!("replace_legal_links (articles): {e}"))?;
    Ok(())
}

/// Dates `VERSIONS_A_VENIR` (ISO) → `NaiveDate` au bord store. La sentinelle
/// `2222-02-22` (date inconnue) est une vraie date, conservée.
fn upcoming_dates(dates: &[String]) -> Result<Vec<chrono::NaiveDate>> {
    dates.iter().map(|d| legi_date(d)).collect()
}

/// Upsert d'un batch de codes LEGI vers `legal_text` (ON CONFLICT `text_uid`) +
/// dates de versions futures (`upcoming_versions`) + remplacement des arêtes
/// texte-niveau (`owner_num_key = ''`). L'upsert texte est inconditionnel, le
/// reste suit.
async fn flush_legi_codes(
    repo: &DecisionRepository<'_>,
    codes: Vec<(
        lj_store::repository::LegalTextRow,
        Vec<chrono::NaiveDate>,
        Vec<lj_store::repository::LegalLinkRow>,
    )>,
) -> Result<()> {
    let mut link_items: Vec<(
        lj_store::repository::LegalLinkOwner,
        Vec<lj_store::repository::LegalLinkRow>,
    )> = Vec::new();
    for (code, upcoming, links) in codes {
        repo.upsert_legal_text(&code)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", code.text_uid))?;
        repo.set_legal_text_upcoming_versions(&code.text_uid, &upcoming)
            .await
            .map_err(|e| anyhow!("set_legal_text_upcoming_versions {}: {e}", code.text_uid))?;
        link_items.push((
            lj_store::repository::LegalLinkOwner {
                text_uid: code.text_uid,
                num_key: String::new(),
                date_debut: None,
            },
            links,
        ));
    }
    repo.replace_legal_links(&link_items)
        .await
        .map_err(|e| anyhow!("replace_legal_links (textes): {e}"))?;
    Ok(())
}

/// Backfill one-shot du graphe `legal_link` : streame des tarballs DILA (stock
/// global puis incréments, dans l'ordre chronologique ; fonds LEGI **et** KALI,
/// discriminés par membre), parse les blocs `<LIENS>` et remplace les arêtes par
/// propriétaire — **sans upsert d'articles**, donc hors gate `content_checksum`.
/// Rejouable. Les `.dat` sont ignorés ; une passe finale purge les arêtes dont le
/// propriétaire n'existe plus en base.
pub async fn backfill_links(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // La purge finale est un anti-join pleine table : batch hors-ligne, pas de timeout.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    for path in paths {
        backfill_links_tarball(&repo, path).await?;
    }
    let pruned = repo
        .prune_orphan_legal_links()
        .await
        .map_err(|e| anyhow!("prune_orphan_legal_links: {e}"))?;
    tracing::info!(tarballs = paths.len(), pruned, "backfill_links done");
    Ok(())
}

/// Passe liens-only sur un tarball : même canal borné que l'ingest, mais le
/// consommateur n'écrit que `legal_link` (`replace_legal_links` par batch).
async fn backfill_links_tarball(repo: &DecisionRepository<'_>, path: &Path) -> Result<()> {
    type Item = (
        lj_store::repository::LegalLinkOwner,
        Vec<lj_store::repository::LegalLinkRow>,
    );
    enum Msg {
        Item(Box<Item>),
        ParseErr,
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Msg>(LEGI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if name.to_lowercase().ends_with(".dat") {
                return Ok(());
            }
            let built: std::result::Result<Option<Item>, String> = match classify_legi_member(&name)
            {
                LegiMember::Article => lj_extract::legi::parse_legi_article(&raw)
                    .map_err(|e| e.to_string())
                    .and_then(|mut art| {
                        let liens = std::mem::take(&mut art.liens);
                        let owner = lj_store::repository::LegalLinkOwner {
                            text_uid: art.legitext,
                            num_key: art.num_key,
                            date_debut: Some(
                                legi_date(&art.date_debut).map_err(|e| e.to_string())?,
                            ),
                        };
                        let rows = legal_link_rows(liens).map_err(|e| e.to_string())?;
                        Ok(Some((owner, rows)))
                    }),
                LegiMember::Texte => lj_extract::legi::parse_legi_texte(&raw)
                    .map_err(|e| e.to_string())
                    .and_then(|mut code| {
                        let liens = std::mem::take(&mut code.liens);
                        let owner = lj_store::repository::LegalLinkOwner {
                            text_uid: code.legitext,
                            num_key: String::new(),
                            date_debut: None,
                        };
                        let rows = legal_link_rows(liens).map_err(|e| e.to_string())?;
                        Ok(Some((owner, rows)))
                    }),
                // Les fichiers de structure ne portent pas de bloc <LIENS>.
                LegiMember::SectionTa | LegiMember::Struct => Ok(None),
                LegiMember::Ignore => match super::kali::classify_kali_member(&name) {
                    super::kali::KaliMember::Article => lj_extract::kali::parse_kali_article(&raw)
                        .map_err(|e| e.to_string())
                        .and_then(|mut art| {
                            let liens = std::mem::take(&mut art.liens);
                            let owner = lj_store::repository::LegalLinkOwner {
                                text_uid: art.kalicont,
                                num_key: art.num_key,
                                date_debut: Some(
                                    legi_date(&art.date_debut).map_err(|e| e.to_string())?,
                                ),
                            };
                            let rows = legal_link_rows(liens).map_err(|e| e.to_string())?;
                            Ok(Some((owner, rows)))
                        }),
                    _ => Ok(None),
                },
            };
            let msg = match built {
                Ok(None) => return Ok(()),
                Ok(Some(item)) => Msg::Item(Box::new(item)),
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "backfill_links: parse failed");
                    Msg::ParseErr
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal backfill fermé (consumer arrêté)"))
        })
    });

    let mut items: Vec<Item> = Vec::new();
    let (mut owners, mut links, mut errors) = (0usize, 0usize, 0usize);
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Item(item) => {
                owners += 1;
                links += item.1.len();
                items.push(*item);
                if items.len() >= LEGI_BATCH_SIZE {
                    let batch = std::mem::take(&mut items);
                    repo.replace_legal_links(&batch)
                        .await
                        .map_err(|e| anyhow!("replace_legal_links: {e}"))?;
                }
            }
            Msg::ParseErr => errors += 1,
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture backfill {}: {e}", path.display()))??;
    if !items.is_empty() {
        repo.replace_legal_links(&items)
            .await
            .map_err(|e| anyhow!("replace_legal_links: {e}"))?;
    }
    tracing::info!(source = %path.display(), owners, links, errors, "backfill_links tarball");
    Ok(())
}

/// Backfill one-shot de l'arbre structurel `legal_toc_edge` : streame des
/// tarballs LEGI (stock global puis incréments), parse les `TEXTELR`/`SECTION_TA`
/// et remplace les arêtes par propriétaire — hors gate `content_checksum`,
/// rejouable. Passe finale : purge des arbres dont le texte n'a plus d'articles en
/// base.
pub async fn backfill_toc(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // La purge finale est un anti-join pleine table : batch hors-ligne, pas de timeout.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    for path in paths {
        backfill_toc_tarball(&repo, path).await?;
    }
    let pruned = repo
        .prune_orphan_toc_edges()
        .await
        .map_err(|e| anyhow!("prune_orphan_toc_edges: {e}"))?;
    tracing::info!(tarballs = paths.len(), pruned, "backfill_toc done");
    Ok(())
}

/// Passe toc-only sur un tarball : même canal borné que l'ingest, mais le
/// consommateur n'écrit que `legal_toc_edge` (`replace_toc_edges` par batch).
async fn backfill_toc_tarball(repo: &DecisionRepository<'_>, path: &Path) -> Result<()> {
    type Item = (
        lj_store::repository::TocOwner,
        Vec<lj_store::repository::TocEdgeRow>,
    );
    enum Msg {
        Item(Box<Item>),
        ParseErr,
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Msg>(LEGI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if name.to_lowercase().ends_with(".dat") {
                return Ok(());
            }
            let parsed = match classify_legi_member(&name) {
                LegiMember::SectionTa => Some(lj_extract::legi::parse_legi_section_ta(&raw)),
                LegiMember::Struct => Some(lj_extract::legi::parse_legi_textelr(&raw)),
                _ => None,
            };
            let msg = match parsed {
                None => return Ok(()),
                Some(p) => match p.map_err(anyhow::Error::from).and_then(legi_toc_item) {
                    Ok(item) => Msg::Item(Box::new(item)),
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "backfill_toc: parse failed");
                        Msg::ParseErr
                    }
                },
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal backfill fermé (consumer arrêté)"))
        })
    });

    let mut items: Vec<Item> = Vec::new();
    let (mut owners, mut edges, mut errors) = (0usize, 0usize, 0usize);
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Item(item) => {
                owners += 1;
                edges += item.1.len();
                items.push(*item);
                if items.len() >= LEGI_BATCH_SIZE {
                    let batch = std::mem::take(&mut items);
                    repo.replace_toc_edges(&batch)
                        .await
                        .map_err(|e| anyhow!("replace_toc_edges: {e}"))?;
                }
            }
            Msg::ParseErr => errors += 1,
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture backfill {}: {e}", path.display()))??;
    if !items.is_empty() {
        repo.replace_toc_edges(&items)
            .await
            .map_err(|e| anyhow!("replace_toc_edges: {e}"))?;
    }
    tracing::info!(source = %path.display(), owners, edges, errors, "backfill_toc tarball");
    Ok(())
}

/// Audits a text's chronicle against what the bulk announces. The `VERSIONS`
/// block of every article file is the source's own manifest of the number, so a
/// version it declares with no row of ours is a hole, and each hole is qualified:
/// the file was in the archives, or a `liste_suppression_legi*.dat` named it, or
/// the bulk never carried it. Reads only.
pub async fn audit_legi_versions(text_uid: &str, paths: &[std::path::PathBuf]) -> Result<()> {
    use std::collections::{HashMap, HashSet};

    let attendu = text_uid.to_string();
    let lots = paths.to_vec();
    let (declare, fichiers, supprimes, lus) = tokio::task::spawn_blocking(move || {
        let mut declare: HashMap<String, HashMap<String, (String, String)>> = HashMap::new();
        let mut fichiers: HashSet<String> = HashSet::new();
        let mut supprimes: HashSet<String> = HashSet::new();
        let mut lus = 0usize;
        for path in &lots {
            lj_sources::tar_reader::for_each_member_filtered(
                path,
                // A suppression list names the audited text nowhere, so the guard has
                // to let it through on its extension alone.
                |name| {
                    name.to_lowercase().ends_with(".dat")
                        || name.split('/').any(|seg| seg == attendu)
                },
                |name, raw| {
                    if name.to_lowercase().ends_with(".dat") {
                        for ligne in String::from_utf8_lossy(&raw).lines() {
                            let stem = ligne.trim().rsplit('/').next().unwrap_or_default();
                            if let Some(id) = stem.strip_suffix(".xml") {
                                supprimes.insert(id.to_string());
                            }
                        }
                        return Ok(());
                    }
                    if !matches!(classify_legi_member(&name), LegiMember::Article) {
                        return Ok(());
                    }
                    let Ok(art) = lj_extract::legi::parse_legi_article(&raw) else {
                        return Ok(());
                    };
                    if art.legitext != attendu {
                        return Ok(());
                    }
                    lus += 1;
                    fichiers.insert(art.legiarti.clone());
                    let chronique = declare.entry(art.num_key.clone()).or_default();
                    for v in &art.versions {
                        chronique.insert(
                            v.legiarti.clone(),
                            (v.date_debut.clone().unwrap_or_default(), v.etat.clone()),
                        );
                    }
                    Ok(())
                },
            )?;
        }
        Ok::<_, anyhow::Error>((declare, fichiers, supprimes, lus))
    })
    .await
    .map_err(|e| anyhow!("tâche lecture audit: {e}"))??;

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let tenu = repo
        .article_version_uids_of(text_uid)
        .await
        .map_err(|e| anyhow!("article_version_uids_of: {e}"))?;
    // Identity is the version id, never `(num_key, id)`: an article renumbered
    // mid-life (`L111-4` becoming `LO111-4` on 1996-07-23) is one chronicle for
    // DILA and two `num_key` for us, and comparing per number calls the second
    // half missing. 704 of the CSS were counted that way before the fix.
    let mut tenu_par_uid: HashMap<&str, &str> = HashMap::new();
    let mut creneaux: HashSet<(&str, &str)> = HashSet::new();
    for (num_key, source_uid, _, debut) in &tenu {
        tenu_par_uid.insert(source_uid.as_str(), num_key.as_str());
        creneaux.insert((num_key.as_str(), debut.as_str()));
    }

    let mut articles_troues = 0usize;
    let mut manquantes = 0usize;
    let mut renumerotees = 0usize;
    let mut dont_supprimees = 0usize;
    let mut dont_fichier_lu = 0usize;
    let mut dont_creneau_occupe = 0usize;
    let mut dont_creneau_occupe_applicable = 0usize;
    let mut exemples: Vec<String> = Vec::new();
    for (num_key, chronique) in &declare {
        let mut absents: Vec<_> = chronique
            .iter()
            .filter(|(id, _)| match tenu_par_uid.get(id.as_str()) {
                Some(porteur) => {
                    if *porteur != num_key.as_str() {
                        renumerotees += 1;
                    }
                    false
                }
                None => true,
            })
            .collect();
        if absents.is_empty() {
            continue;
        }
        absents.sort_by(|a, b| a.1 .0.cmp(&b.1 .0));
        articles_troues += 1;
        manquantes += absents.len();
        for (id, (debut, etat)) in absents {
            let supprime = supprimes.contains(id);
            let fichier = fichiers.contains(id);
            // Same start date as a row we hold: the identity
            // `(text_uid, num_key, date_debut)` has only one slot, so the second
            // version of a day is dropped by the upsert, whatever it carries.
            let occupe = creneaux.contains(&(num_key.as_str(), debut.as_str()));
            dont_supprimees += usize::from(supprime);
            dont_fichier_lu += usize::from(fichier);
            dont_creneau_occupe += usize::from(occupe);
            dont_creneau_occupe_applicable += usize::from(occupe && etat != "MODIFIE_MORT_NE");
            if exemples.len() < 60 {
                exemples.push(format!(
                    "{num_key} {id} debut={debut} etat={etat} supprimee={supprime} fichier_lu={fichier} creneau_occupe={occupe}"
                ));
            }
        }
    }
    let declarees: usize = declare.values().map(HashMap::len).sum();
    let declarees_uid: HashSet<&str> = declare
        .values()
        .flat_map(|c| c.keys().map(String::as_str))
        .collect();
    let orphelines = tenu
        .iter()
        .filter(|(_, source_uid, _, _)| !declarees_uid.contains(source_uid.as_str()))
        .count();
    for ligne in &exemples {
        tracing::info!("chronicle audit: {ligne}");
    }
    tracing::info!(
        texte = %text_uid,
        archives = paths.len(),
        fichiers_lus = lus,
        articles = declare.len(),
        versions_declarees = declarees,
        versions_tenues = tenu.len(),
        articles_troues,
        versions_manquantes = manquantes,
        versions_renumerotees = renumerotees,
        dont_supprimees,
        dont_fichier_lu,
        dont_creneau_occupe,
        dont_creneau_occupe_applicable,
        versions_non_declarees = orphelines,
        "LEGI chronicle audit"
    );
    Ok(())
}

/// Re-applies, from the latest file delivering it under each text that holds
/// it, every LEGI and KALI version the base holds at more than one start date: the writer keeps the delivered date,
/// drops the others with their edges and reopens the ends they capped. Only the
/// archive tells which date is current, the row ids saying nothing once an old
/// stock replay has reinserted a stale date. Counts only, unless `apply`.
pub async fn redate_dila_versions(apply: bool) -> Result<()> {
    use std::collections::{HashMap, HashSet};

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    // The key the daily ingest writes: a KALI collection qualifies its numbers
    // once the base records its head piece.
    let tetes: std::sync::Arc<HashMap<String, String>> = std::sync::Arc::new(
        repo.base_pieces()
            .await
            .map_err(|e| anyhow!("base_pieces: {e}"))?
            .into_iter()
            .collect(),
    );
    let mut touches: HashSet<String> = HashSet::new();
    for (fond, prefixe) in FONDS_VERSIONNES {
        let versions = versions_livrees(&repo, &settings, fond, prefixe, &tetes).await?;
        if !apply {
            continue;
        }
        let mut liens = Vec::new();
        for (row, links) in versions {
            if repo
                .upsert_legal_article(&row, lj_store::repository::Fraicheur::Autoritaire)
                .await
                .map_err(|e| anyhow!("upsert_legal_article {}: {e}", row.source_uid))?
            {
                liens.push((
                    lj_store::repository::LegalLinkOwner {
                        text_uid: row.text_uid.clone(),
                        num_key: row.num_key.clone(),
                        date_debut: row.date_debut,
                    },
                    links,
                ));
            }
            touches.insert(row.text_uid);
        }
        repo.replace_legal_links(&liens)
            .await
            .map_err(|e| anyhow!("replace_legal_links: {e}"))?;
    }
    if !apply {
        return Ok(());
    }
    finir_passe_legi(&repo).await?;
    let liens_orphelins = repo
        .prune_orphan_legal_links()
        .await
        .map_err(|e| anyhow!("prune_orphan_legal_links: {e}"))?;
    for (fond, prefixe) in FONDS_VERSIONNES {
        let restantes = repo
            .versions_redatees(prefixe)
            .await
            .map_err(|e| anyhow!("versions_redatees: {e}"))?
            .into_iter()
            .map(|(uid, ..)| uid)
            .collect::<HashSet<_>>()
            .len();
        tracing::warn!(fond = ?fond, restantes, "DILA versions redated");
    }
    tracing::warn!(
        textes = touches.len(),
        liens_orphelins,
        "DILA redating pass done"
    );
    Ok(())
}

/// Deletes the LEGI versions whose text stands in for an article not yet consolidated,
/// and reopens the JO rows they closed. Counts only, unless `apply`.
pub async fn drop_legi_placeholders(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ids: Vec<i64> = repo
        .legi_placeholder_candidates()
        .await
        .map_err(|e| anyhow!("legi_placeholder_candidates: {e}"))?
        .into_iter()
        .filter(|(_, texte)| lj_extract::legi::is_placeholder(texte))
        .map(|(id, _)| id)
        .collect();
    if !apply {
        tracing::info!(
            placeholders = ids.len(),
            "LEGI placeholders found, none deleted"
        );
        return Ok(());
    }
    let (deleted, reopened) = repo
        .drop_legal_article_versions(&ids)
        .await
        .map_err(|e| anyhow!("drop_legal_article_versions: {e}"))?;
    tracing::warn!(deleted, reopened, "LEGI placeholders dropped");
    Ok(())
}

/// The DILA funds that version their articles, with the prefix of their ids.
const FONDS_VERSIONNES: [(lj_sources::dila::DilaFond, &str); 2] = [
    (lj_sources::dila::DilaFond::Legi, "LEGIARTI"),
    (lj_sources::dila::DilaFond::Kali, "KALIARTI"),
];

/// The latest delivered file of every `fond` version the base holds at several
/// start dates, per text holding it, with its edges: a KALI agreement that
/// hands an avenant over to another keeps listing it. The cache is read newest
/// first until every version is found; a version a suppression list names last
/// is left out.
async fn versions_livrees(
    repo: &DecisionRepository<'_>,
    settings: &Settings,
    fond: lj_sources::dila::DilaFond,
    prefixe: &str,
    tetes: &std::sync::Arc<std::collections::HashMap<String, String>>,
) -> Result<
    Vec<(
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    )>,
> {
    use std::collections::{HashMap, HashSet};

    let paths = lj_sources::dila::cached_dila_tarballs(&settings.cache_dir(), fond)
        .map_err(|e| anyhow!("cached_dila_tarballs {fond:?}: {e}"))?;
    let held = repo
        .versions_redatees(prefixe)
        .await
        .map_err(|e| anyhow!("versions_redatees: {e}"))?;
    let mut dates_tenues: HashMap<(String, String, String), Vec<chrono::NaiveDate>> =
        HashMap::new();
    for (uid, text_uid, num_key, debut) in held {
        dates_tenues
            .entry((uid, text_uid, num_key))
            .or_default()
            .push(debut);
    }
    let mut pending: HashSet<(String, String)> = dates_tenues
        .keys()
        .map(|(uid, text_uid, _)| (uid.clone(), text_uid.clone()))
        .collect();
    let versions = pending.len();
    let mut supprimees = 0usize;
    let mut found = Vec::new();
    for path in paths.iter().rev() {
        if pending.is_empty() {
            break;
        }
        let wanted: HashSet<String> = pending.iter().map(|(uid, _)| uid.clone()).collect();
        let tar_path = path.clone();
        let tetes = std::sync::Arc::clone(tetes);
        let (rows, named) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut rows = Vec::new();
            let mut named: HashSet<String> = HashSet::new();
            lj_sources::tar_reader::for_each_member_filtered(
                &tar_path,
                |name| {
                    name.to_lowercase().ends_with(".dat")
                        || name
                            .rsplit('/')
                            .next()
                            .and_then(|stem| stem.strip_suffix(".xml"))
                            .is_some_and(|uid| wanted.contains(uid))
                },
                |name, raw| {
                    if name.to_lowercase().ends_with(".dat") {
                        for ligne in String::from_utf8_lossy(&raw).lines() {
                            let stem = ligne.trim().rsplit('/').next().unwrap_or_default();
                            if let Some(uid) = stem.strip_suffix(".xml") {
                                if wanted.contains(uid) {
                                    named.insert(uid.to_string());
                                }
                            }
                        }
                        return Ok(());
                    }
                    if let Some(version) = version_livree(fond, &name, &raw, &tetes)? {
                        rows.push(version);
                    }
                    Ok(())
                },
            )?;
            Ok((rows, named))
        })
        .await
        .map_err(|e| anyhow!("redate read task: {e}"))??;
        // Within one archive the delivered file stands over the suppression list.
        for (row, links) in rows {
            if pending.remove(&(row.source_uid.clone(), row.text_uid.clone())) {
                found.push((row, links));
            }
        }
        for uid in named {
            let avant = pending.len();
            pending.retain(|(pending_uid, _)| *pending_uid != uid);
            supprimees += avant - pending.len();
        }
    }
    let (mut lignes_retirees, mut cles_divergentes) = (0usize, 0usize);
    for (row, _) in &found {
        match dates_tenues.get(&(
            row.source_uid.clone(),
            row.text_uid.clone(),
            row.num_key.clone(),
        )) {
            Some(dates) => {
                lignes_retirees += dates.iter().filter(|d| Some(**d) != row.date_debut).count()
            }
            None => {
                cles_divergentes += 1;
                tracing::warn!(
                    source_uid = %row.source_uid,
                    text_uid = %row.text_uid,
                    livree = %row.num_key,
                    date_debut = ?row.date_debut,
                    "delivered version keyed apart from the rows the base holds"
                );
            }
        }
    }
    tracing::warn!(
        fond = ?fond,
        versions,
        found = found.len(),
        supprimees,
        absentes = pending.len(),
        cles_divergentes,
        lignes_retirees,
        "DILA versions held at several start dates"
    );
    Ok(found)
}

/// An archive member as the daily ingest writes it, `None` when it is not an
/// article version.
fn version_livree(
    fond: lj_sources::dila::DilaFond,
    name: &str,
    raw: &[u8],
    tetes: &std::collections::HashMap<String, String>,
) -> Result<
    Option<(
        lj_store::repository::LegalArticleRow,
        Vec<lj_store::repository::LegalLinkRow>,
    )>,
> {
    let checksum = xxhash_rust::xxh3::xxh3_64(raw);
    match fond {
        lj_sources::dila::DilaFond::Legi => {
            if !matches!(classify_legi_member(name), LegiMember::Article) {
                return Ok(None);
            }
            let mut art = lj_extract::legi::parse_legi_article(raw)?;
            if art
                .texte
                .as_deref()
                .is_some_and(lj_extract::legi::is_placeholder)
            {
                return Ok(None);
            }
            let liens = std::mem::take(&mut art.liens);
            Ok(Some((
                legi_article_row(art, checksum)?,
                legal_link_rows(liens)?,
            )))
        }
        lj_sources::dila::DilaFond::Kali => {
            if !matches!(
                super::kali::classify_kali_member(name),
                super::kali::KaliMember::Article
            ) {
                return Ok(None);
            }
            let mut art = lj_extract::kali::parse_kali_article(raw)?;
            let liens = std::mem::take(&mut art.liens);
            let tete = tetes.get(&art.kalicont).map(String::as_str);
            Ok(Some((
                super::kali::kali_article_row(art, checksum, tete)?,
                legal_link_rows(liens)?,
            )))
        }
        autre => anyhow::bail!("{autre:?} holds no article versions"),
    }
}

/// Backfill one-shot des métadonnées de textes : streame des tarballs LEGI et
/// rejoue l'upsert `legal_text` (inconditionnel) + les dates `upcoming_versions`
/// depuis les `TEXTE_VERSION` — textes seulement, ni articles ni liens ni arbre.
/// Rejouable.
pub async fn backfill_textes(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    for path in paths {
        backfill_textes_tarball(&repo, path).await?;
    }
    // Slugs des textes nouveaux : la passe crée des fiches, pas seulement des mises
    // à jour.
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(tarballs = paths.len(), slugged, "backfill_textes done");
    Ok(())
}

/// Passe textes-only sur un tarball : même canal borné que l'ingest, le
/// consommateur upserte `legal_text` + `upcoming_versions` par batch.
async fn backfill_textes_tarball(repo: &DecisionRepository<'_>, path: &Path) -> Result<()> {
    type Item = (lj_store::repository::LegalTextRow, Vec<chrono::NaiveDate>);
    enum Msg {
        Item(Box<Item>),
        ParseErr,
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Msg>(LEGI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if !matches!(classify_legi_member(&name), LegiMember::Texte) {
                return Ok(());
            }
            let built = lj_extract::legi::parse_legi_texte(&raw)
                .map_err(anyhow::Error::from)
                .and_then(|mut code| {
                    let upcoming = upcoming_dates(&std::mem::take(&mut code.versions_a_venir))?;
                    Ok((legi_code_row(code)?, upcoming))
                });
            let msg = match built {
                Ok(item) => Msg::Item(Box::new(item)),
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "backfill_textes: parse failed");
                    Msg::ParseErr
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal backfill fermé (consumer arrêté)"))
        })
    });

    let (mut textes, mut errors) = (0usize, 0usize);
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Item(item) => {
                let (row, upcoming) = *item;
                repo.upsert_legal_text(&row)
                    .await
                    .map_err(|e| anyhow!("upsert_legal_text {}: {e}", row.text_uid))?;
                repo.set_legal_text_upcoming_versions(&row.text_uid, &upcoming)
                    .await
                    .map_err(|e| {
                        anyhow!("set_legal_text_upcoming_versions {}: {e}", row.text_uid)
                    })?;
                textes += 1;
            }
            Msg::ParseErr => errors += 1,
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture backfill {}: {e}", path.display()))??;
    tracing::info!(source = %path.display(), textes, errors, "backfill_textes tarball");
    Ok(())
}

/// Sync incrémental LEGI : télécharge les incréments `LEGI_*.tar.gz` postérieurs
/// au watermark via [`lj_sources::dila::sync_dila`] (ordre lexicographique =
/// chronologique), PUIS ingère chaque incrément et **pose son filigrane derrière
/// lui** — une passe tuée au milieu du lot laisse à reprendre exactement ce
/// qu'elle n'a pas appliqué.
///
/// **Auto-switch cold ↔ warm** : au 1er run (`stock_fetched == false`) `sync_dila`
/// télécharge le **stock global** `Freemium_legi_global_*` (multi-Go) EN PREMIER,
/// puis les incréments postérieurs à sa date ; ensuite il ne renvoie que les
/// nouveaux. Un re-run ne re-télécharge ni ne ré-applique, et l'upsert article
/// skippe les checksums inchangés. `Command::Legi { path }` reste pour ingérer un
/// tarball isolé depuis le disque, sans toucher au filigrane.
pub async fn sync_legi() -> Result<()> {
    let settings = Settings::from_env()?;
    let mut batch =
        lj_sources::dila::sync_dila(&settings.cache_dir(), lj_sources::dila::DilaFond::Legi)
            .map_err(|e| anyhow!("sync_dila legi: {e}"))?;

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

    let (mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    for archive in batch.archives.clone() {
        let (u, s, e) = ingest_legi_tarball(
            &repo,
            &archive.path,
            &[],
            lj_store::repository::Fraicheur::Autoritaire,
        )
        .await?;
        upserted += u;
        skipped += s;
        errors += e;
        batch
            .commit(&archive)
            .map_err(|e| anyhow!("filigrane legi {}: {e}", archive.watermark))?;
    }

    // Collapse des coquilles LEGITEXT vides régénérées par les incréments : une
    // fois, après tous les incréments — le corps JORFTEXT et la coquille LEGITEXT
    // peuvent arriver dans des tarballs différents.
    let collapsed = repo
        .collapse_empty_legitext_doublons()
        .await
        .map_err(|e| anyhow!("collapse_empty_legitext_doublons: {e}"))?;

    // Arbres structurels des textes disparus (suppression `.dat` de tous leurs
    // articles) : purge une fois par sync.
    let toc_pruned = repo
        .prune_orphan_toc_edges()
        .await
        .map_err(|e| anyhow!("prune_orphan_toc_edges: {e}"))?;

    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    finir_passe_legi(&repo).await?;

    tracing::info!(
        increments = batch.archives.len(),
        upserted,
        skipped,
        errors,
        collapsed,
        toc_pruned,
        slugged,
        "sync_legi"
    );
    Ok(())
}
