//! LEGI (référentiel versionné, ADR 0092) : ingest tarball stock/incrément,
//! sync incrémental, résolution citation→article (ADR 0097).

use std::path::Path;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Taille de batch des upserts LEGI (articles puis codes).
const LEGI_BATCH_SIZE: usize = 512;

/// Classe d'un membre XML du fond LEGI, déduite de son chemin (ADR 0092).
///
/// Les `ARTICLE` (`…/article/…/LEGIARTI*.xml`), les `TEXTE_VERSION`
/// (`…/texte/version/…/LEGITEXT*.xml`), les `TEXTELR` (`…/texte/struct/…`) et
/// les `SECTION_TA` (`…/section_ta/…`) sont ingérés (ADR 0092 + 0175) ;
/// l'arbre `eli/` est hors-périmètre.
enum LegiMember {
    Article,
    Texte,
    SectionTa,
    Struct,
    Ignore,
}

/// Classe un membre par son chemin tar. Insensible à la casse de l'extension ;
/// la discrimination se fait sur le nom de fichier (`LEGIARTI`/`LEGITEXT`/
/// `LEGISCTA`) sous le bon dossier.
fn classify_legi_member(name: &str) -> LegiMember {
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
/// au bord store. Erreur franche (#12) si le format est invalide.
fn legi_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date LEGI invalide {iso:?}: {e}"))
}

/// Convertit une [`lj_extract::legi::LegiArticle`] (dates ISO `String`) en
/// [`LegalArticleRow`] (ADR 0112 §1) : `text_uid` = LEGITEXT parent, identité de
/// version `(text_uid, num_key, date_debut)`. Provenance LEGI : `source_uid` =
/// LEGIARTI (le « CID » d'où l'URL Légifrance se dérive, §3). `num_key` (déjà
/// `normalize_article` côté parser pur) et `title_path` (`titre_text`) tels quels.
fn legi_article_row(
    art: lj_extract::legi::LegiArticle,
    content_checksum: u64,
) -> Result<lj_store::repository::LegalArticleRow> {
    let date_fin = match art.date_fin {
        Some(d) => Some(legi_date(&d)?),
        None => None,
    };
    Ok(lj_store::repository::LegalArticleRow {
        text_uid: art.legitext,
        num: art.num,
        num_key: art.num_key,
        // Le tar LEGI n'expose pas l'ordre de lecture du code → `position` non
        // renseignée (lecture « à la suite » repli sur `num_key`, ADR 0112 §9).
        position: None,
        title_path: art.titre_text,
        status: art.etat,
        date_debut: Some(legi_date(&art.date_debut)?),
        date_fin,
        texte: art.texte,
        // Droit FR officiel, monolingue (ADR 0116).
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: art.nota,
        content_checksum,
        source: "legifrance".to_string(),
        source_uid: art.legiarti,
        source_url: None,
        // Source *live* autoritaire : fraîcheur dérivée de `ingest_freshness` (ADR 0129),
        // pas stockée par ligne.
        source_asof: None,
        source_upstream_url: None,
    })
}

/// Direction d'un lien (ADR 0174), **relative au propriétaire** — l'attribut DILA
/// `sens` étant inexploitable (cf. `lj_extract::legi::collect_liens`). L'auteur
/// d'une modification/citation est toujours le texte le plus récent :
/// - cible sans date (article de code consolidé, sentinelle absorbée) → le
///   propriétaire agit dessus → `outgoing` ;
/// - propriétaire = article de code (`LEGITEXT`) → il subit l'action d'un texte
///   daté (un code n'est jamais l'auteur d'une modif) → `incoming` ;
/// - propriétaire = texte daté (loi/décret/convention) → `incoming` ssi la cible
///   est plus récente que lui (elle agit), `outgoing` sinon (elle est plus
///   ancienne : le propriétaire la modifie ou la cite).
fn link_direction(
    owner: &lj_store::repository::LegalLinkOwner,
    target_date: Option<chrono::NaiveDate>,
) -> &'static str {
    match target_date {
        None => "outgoing",
        Some(_) if owner.text_uid.starts_with("LEGITEXT") => "incoming",
        Some(td) if owner.date_debut.is_some_and(|od| td > od) => "incoming",
        Some(_) => "outgoing",
    }
}

/// Convertit les [`lj_extract::legi::LegiLien`] d'un fichier en lignes
/// [`LegalLinkRow`] (ADR 0174) au bord store : dates parsées (sentinelles déjà
/// absorbées côté parser), direction calculée depuis l'`owner` ([`link_direction`]),
/// `target_num_key` = `normalize_article` pour la résolution des cibles article
/// sans ID. Partagé avec le pipeline KALI (même bloc `<LIENS>`).
pub(crate) fn legal_link_rows(
    owner: &lj_store::repository::LegalLinkOwner,
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
                    lj_core::article_key::article_key(&lj_extract::extract::normalize_article(n))
                })
                .filter(|k| !k.is_empty());
            Ok(lj_store::repository::LegalLinkRow {
                typelien: l.typelien,
                verb: l.verb,
                direction: link_direction(owner, target_date).to_string(),
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

/// Convertit un [`lj_extract::legi::LegiToc`] (arbre structurel, ADR 0207) en
/// item d'écriture `legal_toc_edge` au bord store : dates parsées, `seq`
/// implicite (ordre du `Vec`).
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

/// Propriétaires `legal_toc_edge` visés par une liste de suppression `.dat`
/// (ADR 0207) : les stems `LEGISCTA` (versions de section) des chemins
/// `section_ta` — un fichier = un propriétaire, purge précise. Les racines
/// (`texte/struct`, owner = cid partagé entre versions) ne sont **pas**
/// purgées ici : la suppression d'un texte entier passe par
/// `prune_orphan_toc_edges` (plus d'articles ⇒ plus d'arbre).
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
/// [`LegalTextRow`] `jurisdiction='FR'` (ADR 0112 §1). `text_uid` = CID
/// chronique (identité partagée avec les articles et la TOC, ADR 0225),
/// `title` = TITREFULL descriptif quand présent (le TITRE d'un TNC est nu),
/// `title_key` = `normalize_instrument(titre)` (posé côté Rust à la frontière
/// d'ingest). La provenance vit sur les versions.
fn legi_code_row(code: lj_extract::legi::LegiCode) -> Result<lj_store::repository::LegalTextRow> {
    let last_modified = match code.derniere_modification {
        Some(d) => Some(legi_date(&d)?),
        None => None,
    };
    let title = code.titre_full.unwrap_or(code.titre);
    let title_key = lj_extract::extract::normalize_instrument(&title);
    Ok(lj_store::repository::LegalTextRow {
        text_uid: code.legitext,
        jurisdiction: "FR".to_string(),
        title,
        title_key,
        nature: code.nature,
        last_modified,
        // Un code n'a pas de date de signature unique ; date_publi non portée par LEGI.
        date_texte: None,
        date_publi: None,
        // Identité d'un code = son slug, pas la cascade ADR 0115 (ni date ni
        // numéro) — mais le NOR des actes datés (arrêtés, décrets) sert de
        // clé de résolution au linker.
        eli: None,
        nor: code.nor,
        instrument_key: None,
        body: None,
        status: None,
    })
}

/// Ingère un stock ou un incrément LEGI (`tar.gz` bulk DILA, ADR 0092).
///
/// Streaming des membres (`tar_reader::for_each_member`) : les `LEGIARTI*.xml`
/// sont parsés en [`lj_extract::legi::parse_legi_article`], les `LEGITEXT*.xml` (sous
/// `texte/version/`) en [`lj_extract::legi::parse_legi_texte`] ; le reste est ignoré.
/// `content_checksum` = xxh3-64 des octets bruts du membre (idempotence #7) ;
/// l'upsert article skippe les lignes au checksum inchangé. Les suppressions
/// (`liste_suppression_legi*.dat`) sont appliquées après les upserts via
/// `delete_legal_articles_by_paths` (canal séparé). `payload_format = 'dila-xml'`.
///
/// `path` = chemin local d'un `Freemium_legi_global_*.tar.gz` (stock) ou d'un
/// incrément `LEGI_*.tar.gz` (téléchargement hors-bande, ADR 0093).
pub async fn ingest_legi(path: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    ingest_legi_tarball(&repo, path).await?;
    let collapsed = repo
        .collapse_empty_legitext_doublons()
        .await
        .map_err(|e| anyhow!("collapse_empty_legitext_doublons: {e}"))?;
    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(collapsed, slugged, "ingest_legi collapse doublons");
    Ok(())
}

/// Ingère un unique `tar.gz` LEGI via `repo` (boucle membres → parse → upsert +
/// suppressions `.dat`). Helper partagé par [`ingest_legi`] (bootstrap manuel
/// d'un tarball) et [`sync_legi`] (chaque incrément téléchargé). Renvoie
/// `(upserted, skipped, errors)`. Idempotent par `content_checksum` (#7).
async fn ingest_legi_tarball(
    repo: &DecisionRepository<'_>,
    path: &Path,
) -> Result<(usize, usize, usize)> {
    // Type des messages remontés par le lecteur (membres parsés). Rows boxées
    // (variantes de tailles très inégales). Chaque row voyage avec les arêtes
    // de son bloc <LIENS> (ADR 0174).
    enum LegiMsg {
        Article(
            Box<(
                lj_store::repository::LegalArticleRow,
                Vec<lj_store::repository::LegalLinkRow>,
            )>,
        ),
        Code(
            Box<(
                lj_store::repository::LegalTextRow,
                Vec<chrono::NaiveDate>,
                Vec<lj_store::repository::LegalLinkRow>,
            )>,
        ),
        Toc(
            Box<(
                lj_store::repository::TocOwner,
                Vec<lj_store::repository::TocEdgeRow>,
            )>,
        ),
        Dat(Vec<String>),
        ParseErr,
    }

    // Lecture+parsing (gzip+tar+XML = CPU sync) en thread bloquant, BORNÉE par un
    // canal : RAM ~constante (le stock global a 1,5 M+ membres → JAMAIS tout en
    // mémoire, cf. tar_reader::for_each_member). Le DB reste côté async — le `repo`
    // emprunte la connexion, non déplaçable dans spawn_blocking. Backpressure : si
    // le canal est plein (flush DB en cours), le lecteur attend sur blocking_send.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<LegiMsg>(LEGI_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
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
                    Ok(mut art) => {
                        let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                        let liens = std::mem::take(&mut art.liens);
                        let built = legi_article_row(art, checksum).and_then(|row| {
                            let owner = lj_store::repository::LegalLinkOwner {
                                text_uid: row.text_uid.clone(),
                                num_key: row.num_key.clone(),
                                date_debut: row.date_debut,
                            };
                            Ok((row, legal_link_rows(&owner, liens)?))
                        });
                        match built {
                            Ok(pair) => LegiMsg::Article(Box::new(pair)),
                            Err(e) => {
                                tracing::error!(member = %name, error = %e, "legi article: row invalide");
                                LegiMsg::ParseErr
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "legi article: parse échec");
                        LegiMsg::ParseErr
                    }
                },
                LegiMember::Texte => match lj_extract::legi::parse_legi_texte(&raw) {
                    Ok(mut code) => {
                        let liens = std::mem::take(&mut code.liens);
                        let upcoming = std::mem::take(&mut code.versions_a_venir);
                        let built = legi_code_row(code).and_then(|row| {
                            let owner = lj_store::repository::LegalLinkOwner {
                                text_uid: row.text_uid.clone(),
                                num_key: String::new(),
                                date_debut: None,
                            };
                            Ok((
                                row,
                                upcoming_dates(&upcoming)?,
                                legal_link_rows(&owner, liens)?,
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
                        tracing::error!(member = %name, error = %e, "legi texte: parse échec");
                        LegiMsg::ParseErr
                    }
                },
                LegiMember::SectionTa => match lj_extract::legi::parse_legi_section_ta(&raw)
                    .map_err(anyhow::Error::from)
                    .and_then(legi_toc_item)
                {
                    Ok(item) => LegiMsg::Toc(Box::new(item)),
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "legi section_ta: parse échec");
                        LegiMsg::ParseErr
                    }
                },
                LegiMember::Struct => match lj_extract::legi::parse_legi_textelr(&raw)
                    .map_err(anyhow::Error::from)
                    .and_then(legi_toc_item)
                {
                    Ok(item) => LegiMsg::Toc(Box::new(item)),
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "legi textelr: parse échec");
                        LegiMsg::ParseErr
                    }
                },
                LegiMember::Ignore => return Ok(()),
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal LEGI fermé (consumer arrêté)"))
        })
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

    while let Some(msg) = rx.recv().await {
        match msg {
            LegiMsg::Article(row) => {
                articles.push(*row);
                if articles.len() >= LEGI_BATCH_SIZE {
                    let batch = std::mem::take(&mut articles);
                    flush_legi_articles(repo, batch, &mut upserted, &mut skipped).await?;
                }
            }
            LegiMsg::Code(row) => {
                codes.push(*row);
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
            LegiMsg::ParseErr => errors += 1,
        }
    }
    // Canal fermé = lecteur fini : remonter une éventuelle erreur de lecture/parse.
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture LEGI {}: {e}", path.display()))??;

    if !articles.is_empty() {
        flush_legi_articles(repo, articles, &mut upserted, &mut skipped).await?;
    }
    if !codes.is_empty() {
        flush_legi_codes(repo, codes).await?;
    }
    if !tocs.is_empty() {
        repo.replace_toc_edges(&tocs)
            .await
            .map_err(|e| anyhow!("replace_toc_edges: {e}"))?;
    }

    // Suppressions appliquées après les upserts (un incrément ajoute ET retire, #7).
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

    // Pose `code_title` (titre du code dénormalisé → titre formé `search_title`,
    // ADR 0114) sur les articles dont il diffère : LEGI streame articles et codes
    // séparément, l'article n'a pas le titre du code au parse.
    let retitled = repo
        .refresh_article_denorm()
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;

    tracing::info!(
        source = %path.display(),
        upserted,
        skipped,
        deleted,
        retitled,
        errors,
        "ingest_legi"
    );
    Ok((upserted, skipped, errors))
}

/// Upsert d'un batch d'articles LEGI vers `referential_articles` (idempotent #7) ;
/// cumule modifiés/skippés. Les arêtes `legal_link` des articles réellement
/// écrits sont remplacées dans la foulée (checksum inchangé ⇒ liens inchangés,
/// on ne les réécrit pas — ADR 0174).
async fn flush_legi_articles(
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
        .map_err(|e| anyhow!("replace_legal_links (articles): {e}"))?;
    Ok(())
}

/// Dates `VERSIONS_A_VENIR` (ISO) → `NaiveDate` au bord store (ADR 0178). La
/// sentinelle `2222-02-22` (date inconnue) est une vraie date, conservée.
fn upcoming_dates(dates: &[String]) -> Result<Vec<chrono::NaiveDate>> {
    dates.iter().map(|d| legi_date(d)).collect()
}

/// Upsert d'un batch de codes LEGI vers `legal_text` (ON CONFLICT `text_uid`) +
/// dates de versions futures (`upcoming_versions`, ADR 0178) + remplacement des
/// arêtes texte-niveau (`owner_num_key = ''`, ADR 0174) — l'upsert texte est
/// inconditionnel, le reste suit.
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

/// Backfill one-shot du graphe `legal_link` (ADR 0174) : streame des tarballs
/// DILA (stock global puis incréments, dans l'ordre chronologique ; fonds LEGI
/// **et** KALI, discriminés par membre), parse les blocs `<LIENS>` et remplace
/// les arêtes par propriétaire — **sans upsert d'articles**, donc hors gate
/// `content_checksum`. Rejouable (remplacement). Les `.dat` sont ignorés (les
/// suppressions ont déjà été rejouées par le sync) : une passe finale purge les
/// arêtes dont le propriétaire n'existe plus en base.
pub async fn backfill_links(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // La purge finale est un anti-join pleine table : batch hors-ligne, pas de timeout.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
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
    tracing::info!(tarballs = paths.len(), pruned, "backfill_links terminé");
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
                        let rows = legal_link_rows(&owner, liens).map_err(|e| e.to_string())?;
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
                        let rows = legal_link_rows(&owner, liens).map_err(|e| e.to_string())?;
                        Ok(Some((owner, rows)))
                    }),
                // Les fichiers de structure ne portent pas de bloc <LIENS>.
                LegiMember::SectionTa | LegiMember::Struct => Ok(None),
                LegiMember::Ignore => match super::kali::classify_kali_member(&name) {
                    super::kali::KaliMember::Article => {
                        lj_extract::kali::parse_kali_article(&raw)
                            .map_err(|e| e.to_string())
                            .and_then(|mut art| {
                                // Article KALI sans numéro : non ingéré → pas d'owner.
                                if art.num_key.is_empty() {
                                    return Ok(None);
                                }
                                let liens = std::mem::take(&mut art.liens);
                                let owner = lj_store::repository::LegalLinkOwner {
                                    text_uid: art.kalicont,
                                    num_key: art.num_key,
                                    date_debut: Some(
                                        legi_date(&art.date_debut).map_err(|e| e.to_string())?,
                                    ),
                                };
                                let rows =
                                    legal_link_rows(&owner, liens).map_err(|e| e.to_string())?;
                                Ok(Some((owner, rows)))
                            })
                    }
                    _ => Ok(None),
                },
            };
            let msg = match built {
                Ok(None) => return Ok(()),
                Ok(Some(item)) => Msg::Item(Box::new(item)),
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "backfill_links: parse échec");
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

/// Backfill one-shot de l'arbre structurel `legal_toc_edge` (ADR 0207) :
/// streame des tarballs LEGI (stock global puis incréments), parse les
/// `TEXTELR`/`SECTION_TA` et remplace les arêtes par propriétaire — hors gate
/// `content_checksum`, rejouable. Passe finale : purge des arbres dont le
/// texte n'a plus d'articles en base.
pub async fn backfill_toc(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // La purge finale est un anti-join pleine table : batch hors-ligne, pas de timeout.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
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
    tracing::info!(tarballs = paths.len(), pruned, "backfill_toc terminé");
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
                        tracing::error!(member = %name, error = %e, "backfill_toc: parse échec");
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

/// Backfill one-shot des métadonnées de textes (ADR 0178) : streame des
/// tarballs LEGI et rejoue l'upsert `legal_text` (inconditionnel) + les dates
/// `upcoming_versions` depuis les `TEXTE_VERSION` — textes seulement, ni
/// articles ni liens ni arbre. Rejouable.
pub async fn backfill_textes(paths: &[std::path::PathBuf]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    for path in paths {
        backfill_textes_tarball(&repo, path).await?;
    }
    // Slugs des textes nouveaux (ADR 0162) — la passe crée des fiches
    // (ré-ancrage CID, ADR 0225), pas seulement des mises à jour.
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(tarballs = paths.len(), slugged, "backfill_textes terminé");
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
                    tracing::error!(member = %name, error = %e, "backfill_textes: parse échec");
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

/// Sync incrémental LEGI (référentiel versionné, ADR 0092/0093) : télécharge les
/// incréments `LEGI_*.tar.gz` postérieurs au watermark via
/// [`lj_sources::dila::sync_dila`] (même mécanisme manifeste/watermark que les
/// autres fonds DILA, ordre lexicographique = chronologique), PUIS ingère chaque
/// incrément fraîchement téléchargé via [`ingest_legi_tarball`].
///
/// **Auto-switch cold ↔ warm** : au 1er run (`stock_fetched == false`) `sync_dila`
/// télécharge le **stock global** `Freemium_legi_global_*` (multi-Go) EN PREMIER,
/// cale le watermark sur sa date, puis enchaîne les incréments ; ensuite il ne
/// renvoie que les nouveaux. Le stock est donc ingéré comme n'importe quel
/// tarball renvoyé (`ingest_legi_tarball`). Un re-run ne re-télécharge ni ne
/// ré-applique (≤ watermark ignoré), et l'upsert article skippe les checksums
/// inchangés (#7). `Command::Legi { path }` reste pour ingérer un tarball isolé
/// depuis le disque (dev/reprise), sans toucher au watermark.
pub async fn sync_legi() -> Result<()> {
    let settings = Settings::from_env()?;
    let downloaded =
        lj_sources::dila::sync_dila(&settings.cache_dir(), lj_sources::dila::DilaFond::Legi)
            .map_err(|e| anyhow!("sync_dila legi: {e}"))?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    for path in &downloaded {
        let (u, s, e) = ingest_legi_tarball(&repo, path).await?;
        upserted += u;
        skipped += s;
        errors += e;
    }

    // Collapse des coquilles LEGITEXT vides régénérées par les incréments (ADR 0115
    // §2) : une fois, après tous les incréments (le corps JORFTEXT et la coquille
    // LEGITEXT peuvent arriver dans des tarballs différents).
    let collapsed = repo
        .collapse_empty_legitext_doublons()
        .await
        .map_err(|e| anyhow!("collapse_empty_legitext_doublons: {e}"))?;

    // Arbres structurels des textes disparus (suppression `.dat` de tous leurs
    // articles) : purge une fois par sync (ADR 0207).
    let toc_pruned = repo
        .prune_orphan_toc_edges()
        .await
        .map_err(|e| anyhow!("prune_orphan_toc_edges: {e}"))?;

    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;

    tracing::info!(
        increments = downloaded.len(),
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

#[cfg(test)]
mod tests {
    use super::link_direction;
    use lj_store::repository::LegalLinkOwner;

    fn owner(text_uid: &str, date: Option<&str>) -> LegalLinkOwner {
        LegalLinkOwner {
            text_uid: text_uid.to_string(),
            num_key: "x".to_string(),
            date_debut: date.map(|d| d.parse().expect("date owner")),
        }
    }

    fn td(d: &str) -> Option<chrono::NaiveDate> {
        Some(d.parse().expect("target date"))
    }

    /// Cible sans date (article de code consolidé, sentinelle absorbée) : le
    /// propriétaire agit dessus → sortant, quel que soit le propriétaire.
    #[test]
    fn undated_target_is_outgoing() {
        assert_eq!(
            link_direction(&owner("JORFTEXT000", Some("2004-01-01")), None),
            "outgoing"
        );
        assert_eq!(
            link_direction(&owner("LEGITEXT000", None), None),
            "outgoing"
        );
    }

    /// Propriétaire = article de code (LEGITEXT) + cible datée : le code subit
    /// toujours l'action d'un texte daté (jamais auteur d'une modif) → entrant.
    /// Cas C. civ. 16-13 ← loi 2004-800 (la date de la loi ≈ date de la version).
    #[test]
    fn code_owner_dated_target_is_incoming() {
        let civ = owner("LEGITEXT000006070721", Some("2004-08-07"));
        assert_eq!(link_direction(&civ, td("2004-08-06")), "incoming");
    }

    /// Propriétaire = texte daté (loi/JORF) : entrant ssi la cible est plus
    /// récente (elle agit sur le propriétaire), sortant si plus ancienne.
    #[test]
    fn dated_owner_uses_recency() {
        let ord = owner("JORFTEXT000032004939", Some("2016-02-11"));
        // Modifié par une loi plus récente (2018) → entrant.
        assert_eq!(link_direction(&ord, td("2018-04-20")), "incoming");
        // Cite un décret plus ancien (1989) → sortant.
        assert_eq!(link_direction(&ord, td("1989-09-06")), "outgoing");
    }

    /// Propriétaire daté sans date connue (sentinelle non résolue) → sortant par
    /// défaut (comparaison temporelle impossible).
    #[test]
    fn dated_owner_without_date_defaults_outgoing() {
        assert_eq!(
            link_direction(&owner("JORFTEXT000", None), td("2018-04-20")),
            "outgoing"
        );
    }
}
