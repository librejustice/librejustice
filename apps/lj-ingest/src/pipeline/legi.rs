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
/// Les `ARTICLE` (`…/article/…/LEGIARTI*.xml`) et les `TEXTE_VERSION`
/// (`…/texte/version/…/LEGITEXT*.xml`) sont ingérés ; `section_ta`, `texte/struct`
/// et l'arbre `eli/` sont hors-périmètre v1 (le fil d'Ariane vient du `TM` porté
/// par chaque ARTICLE).
enum LegiMember {
    Article,
    Texte,
    Ignore,
}

/// Classe un membre par son chemin tar. Insensible à la casse de l'extension ;
/// la discrimination article/texte se fait sur le nom de fichier (`LEGIARTI` vs
/// `LEGITEXT`) sous le bon dossier.
fn classify_legi_member(name: &str) -> LegiMember {
    let lower = name.to_lowercase();
    let stem = name.rsplit('/').next().unwrap_or(name);
    if lower.contains("/eli/") || lower.contains("section_ta") || lower.contains("/struct/") {
        return LegiMember::Ignore;
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

/// Convertit une [`lj_extract::legi::LegiCode`] (dates ISO `String`) en
/// [`LegalTextRow`] `jurisdiction='FR'` (ADR 0112 §1). `text_uid` = LEGITEXT
/// (identité), `title_key` = `normalize_instrument(titre)`
/// (posé côté Rust à la frontière d'ingest). La provenance vit sur les versions.
fn legi_code_row(code: lj_extract::legi::LegiCode) -> Result<lj_store::repository::LegalTextRow> {
    let last_modified = match code.derniere_modification {
        Some(d) => Some(legi_date(&d)?),
        None => None,
    };
    let title_key = lj_extract::extract::normalize_instrument(&code.titre);
    Ok(lj_store::repository::LegalTextRow {
        text_uid: code.legitext,
        jurisdiction: "FR".to_string(),
        title: code.titre,
        title_key,
        nature: code.nature,
        last_modified,
        // Un code n'a pas de date de signature unique ; date_publi non portée par LEGI.
        date_texte: None,
        date_publi: None,
        // Identité d'un code = son slug, pas la cascade ADR 0115 (ni date ni numéro).
        eli: None,
        nor: None,
        instrument_key: None,
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
    // (variantes de tailles très inégales).
    enum LegiMsg {
        Article(Box<lj_store::repository::LegalArticleRow>),
        Code(Box<lj_store::repository::LegalTextRow>),
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
                    Ok(art) => {
                        let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                        match legi_article_row(art, checksum) {
                            Ok(row) => LegiMsg::Article(Box::new(row)),
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
                    Ok(code) => match legi_code_row(code) {
                        Ok(row) => LegiMsg::Code(Box::new(row)),
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "legi texte: row invalide");
                            LegiMsg::ParseErr
                        }
                    },
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "legi texte: parse échec");
                        LegiMsg::ParseErr
                    }
                },
                LegiMember::Ignore => return Ok(()),
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal LEGI fermé (consumer arrêté)"))
        })
    });

    let mut articles: Vec<lj_store::repository::LegalArticleRow> = Vec::new();
    let mut codes: Vec<lj_store::repository::LegalTextRow> = Vec::new();
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

    // Suppressions appliquées après les upserts (un incrément ajoute ET retire, #7).
    let mut deleted: u64 = 0;
    for paths in dat_paths {
        deleted += repo
            .delete_legal_articles_by_paths("legifrance", &paths)
            .await
            .map_err(|e| anyhow!("delete_legal_articles_by_paths: {e}"))?;
    }

    // Pose `code_title` (titre du code dénormalisé → titre formé `search_title`,
    // ADR 0114) sur les articles dont il diffère : LEGI streame articles et codes
    // séparément, l'article n'a pas le titre du code au parse.
    let retitled = repo
        .refresh_article_code_titles()
        .await
        .map_err(|e| anyhow!("refresh_article_code_titles: {e}"))?;

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
/// cumule modifiés/skippés.
async fn flush_legi_articles(
    repo: &DecisionRepository<'_>,
    articles: Vec<lj_store::repository::LegalArticleRow>,
    upserted: &mut usize,
    skipped: &mut usize,
) -> Result<()> {
    for art in articles {
        if repo
            .upsert_legal_article(&art)
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

/// Upsert d'un batch de codes LEGI vers `legal_text` (ON CONFLICT `text_uid`).
async fn flush_legi_codes(
    repo: &DecisionRepository<'_>,
    codes: Vec<lj_store::repository::LegalTextRow>,
) -> Result<()> {
    for code in codes {
        repo.upsert_legal_text(&code)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", code.text_uid))?;
    }
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

    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;

    tracing::info!(
        increments = downloaded.len(),
        upserted,
        skipped,
        errors,
        collapsed,
        slugged,
        "sync_legi"
    );
    Ok(())
}
