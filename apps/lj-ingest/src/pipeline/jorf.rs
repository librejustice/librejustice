//! JORF (Journal officiel, bulk DILA, ADR 0109) : ingest tarball stock/incrément
//! du fond complet → référentiel versionné `referential_texts`/`referential_articles`.
//!
//! Le fond JORF partage la structure XML de LEGI (ADR 0109 §2, audit working-note
//! 2026-06-17) : on réutilise le parser pur factorisé [`lj_extract::jorf`]. Deux
//! divergences pilotent ce pipeline :
//! - **Tagging traité par la juridiction** : les textes détectés comme accords/traités
//!   ([`lj_extract::jorf::is_treaty`]) sont taggés `jurisdiction='INTL'`,
//!   `nature='TRAITE'` (ADR 0109 §1) ; tout le reste du JO en `jurisdiction='FR'`.
//!   Le `source` reste le diffuseur `jorf` (DILA) des deux côtés (ADR 0131 : `source`
//!   = diffuseur, pas catégorie). D'où une ingestion **en deux passes** : 1) les textes
//!   (qui révèlent les `cid` de traités), 2) les articles (seuls ceux dont le `cid`
//!   parent est un traité sont persistés).
//! - **Articles sans numéro ignorés** : JORF contient des `JORFARTI` `NUM` vide
//!   (annonces, actes `TYPE=AUTONOME`) ; non citables par numéro d'article, ils
//!   n'alimentent pas le référentiel versionné (seuls les articles numérotés le
//!   sont). Les `referential_texts` restent, eux, complets.
//!
//! Le corps des accords **anciens** (franco-algérien 1968 & co) est absent du bulk
//! (pré-numérisation) : leurs `referential_articles` viennent d'un track distinct
//! (OCR/extraction hors bulk, ADR 0109 Addendum), pas d'ici.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Taille de batch des upserts JORF (textes puis articles).
const JORF_BATCH_SIZE: usize = 512;

/// Classe d'un membre XML du fond JORF, déduite de son chemin (ADR 0109 §2,
/// calque [`super::legi`]). Seuls `texte/version` (TEXTE_VERSION) et `article`
/// (JORFARTI) sont ingérés ; `texte/struct`, `conteneur` (sommaires JO) et `eli/`
/// sont hors-périmètre (le fil d'Ariane vient du `TM` porté par chaque article).
enum JorfMember {
    Article,
    Texte,
    Ignore,
}

/// Classe un membre par son chemin tar.
fn classify_jorf_member(name: &str) -> JorfMember {
    let lower = name.to_lowercase();
    let stem = name.rsplit('/').next().unwrap_or(name);
    if lower.contains("/struct/") || lower.contains("/conteneur/") || lower.contains("/eli/") {
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

/// Convertit une date ISO (`YYYY-MM-DD`) émise par le parser pur en `NaiveDate`.
/// Erreur franche (#12) si le format est invalide.
fn jorf_date(iso: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map_err(|e| anyhow!("date JORF invalide {iso:?}: {e}"))
}

/// Message de la passe textes : la ligne, son statut traité, et ses liens
/// texte-niveau (graphe ADR 0246 — écrits pour les fiches nouvelles).
struct JorfTexteMsg {
    row: lj_store::repository::LegalTextRow,
    treaty: bool,
    liens: Vec<lj_extract::jorf::JorfLien>,
}

/// Convertit un [`lj_extract::jorf::JorfTexte`] en [`JorfTexteMsg`] (ADR 0112 §1).
/// Traité détecté ⇒ `jurisdiction='INTL'`/`nature='TRAITE'` ; sinon
/// `jurisdiction='FR'`/`nature` du JO (`source` ayant quitté l'identité, c'est la
/// juridiction qui porte le clivage traité ↔ JO — `treaty_text_uids`). `title` = le
/// libellé descriptif `TITREFULL` si présent (pour que `title_key` matche les
/// citations), à défaut `TITRE` court. `date_texte`/`date_publi` natifs du JO.
fn jorf_texte_msg(mut t: lj_extract::jorf::JorfTexte) -> JorfTexteMsg {
    let liens = std::mem::take(&mut t.liens);
    let treaty = lj_extract::jorf::is_treaty(&t);
    // Cascade d'identité ADR 0115 (capturée avant les moves de `t`). `instrument_key`
    // sur la nature d'origine (pas "TRAITE"), filet pour les actes numérotés sans ELI/NOR.
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
        jurisdiction: if treaty { "INTL" } else { "FR" }.to_string(),
        title,
        title_key,
        nature: if treaty {
            "TRAITE".to_string()
        } else {
            t.nature
        },
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
    JorfTexteMsg { row, treaty, liens }
}

/// Convertit les [`lj_extract::jorf::JorfLien`] d'un texte en lignes
/// [`LegalLinkRow`] (ADR 0174, même modèle que LEGI/KALI) : `verb` par le
/// mapping `typelien` partagé, direction lue du `sens` DILA (`source` = la
/// cible agit sur l'owner), cible en clé pendante résolue au read-time.
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
                    lj_core::article_key::article_key(&lj_extract::extract::normalize_article(n))
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
/// hors référentiel versionné). `source='jorf'` (diffuseur DILA) : l'appelant ne route
/// ici que les articles dont le `cid` parent est un traité (ADR 0115 §5, le reste
/// du JO n'est plus persisté) ; la nature « traité » est portée par le texte parent.
fn jorf_article_row(
    art: lj_extract::jorf::JorfArticle,
    content_checksum: u64,
) -> Result<Option<lj_store::repository::LegalArticleRow>> {
    let (Some(num), Some(num_key)) = (art.num, art.num_key) else {
        return Ok(None);
    };
    // Diffuseur = le Journal officiel (DILA), ADR 0131 : `source` est un libellé de
    // diffuseur, pas une catégorie. La nature « traité » est portée par
    // `jurisdiction='INTL'`/`nature='TRAITE'` du texte parent, pas par `source`.
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
        // Droit FR/traités au JO : texte officiel, monolingue côté base (ADR 0116).
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: art.nota,
        content_checksum,
        source,
        source_uid: art.jorfarti,
        source_url: None,
        // Bulk DILA (ingest manuel/périodique, PAS re-sync quotidien) → fraîcheur = la
        // **date de get** (date d'ingest), stable par ligne. ADR 0129 : ne jamais laisser
        // la fraîcheur effective inconnue ; jorf/treaty ne dérivent pas du live.
        source_asof: Some(chrono::Utc::now().date_naive()),
        source_upstream_url: None,
    }))
}

/// Ingère un stock ou un incrément JORF (`tar.gz` bulk DILA, ADR 0109) en deux
/// passes sur l'archive :
/// 1. **textes** → `referential_texts` (tag `treaty`/`jorf`) ;
/// 2. **articles** → `referential_articles`, taggés selon les `cid` de traités
///    (lus en base entre les passes : ce stock + tout traité d'incrément antérieur).
///
/// `path` = chemin local d'un `Freemium_jorf_global_*.tar.gz` (stock) ou d'un
/// incrément `JORF_*.tar.gz`. Streaming borné par canal (RAM ~constante, le stock
/// global a plusieurs M de membres), comme [`super::legi`].
pub async fn ingest_jorf(path: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Finisseurs pleine-table (1,4 M de titres) : au-delà du statement_timeout
    // de 30 s du pool.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    ingest_jorf_archive(&repo, path).await?;
    finish_jorf(&repo).await
}

/// Les deux passes d'une archive (textes puis articles traités). Les finisseurs
/// globaux ([`finish_jorf`]) sont à la charge de l'appelant : une fois par
/// archive en manuel, une fois par run en sync (un rattrapage applique des
/// centaines d'incréments).
async fn ingest_jorf_archive(repo: &DecisionRepository<'_>, path: &Path) -> Result<()> {
    // Passe 1 : textes (révèle les cid de traités).
    let texts = ingest_jorf_texts(repo, path).await?;

    // Set complet des cid de traités en base (ce stock + antérieurs).
    let treaty_cids: HashSet<String> = repo
        .treaty_text_uids()
        .await
        .map_err(|e| anyhow!("treaty_text_uids: {e}"))?
        .into_iter()
        .collect();

    // Passe 2 : articles (taggés treaty/jorf selon le cid parent).
    let (upserted, skipped, no_num, errors) =
        ingest_jorf_articles(repo, path, Arc::new(treaty_cids.clone())).await?;

    tracing::info!(
        source = %path.display(),
        treaties_upserted = texts.treaties,
        texts_created = texts.created,
        texts_existing = texts.existing,
        text_links = texts.links,
        treaties = treaty_cids.len(),
        articles_upserted = upserted,
        articles_skipped = skipped,
        articles_no_num = no_num,
        errors,
        "ingest_jorf_archive"
    );
    Ok(())
}

/// Finisseurs globaux post-archives : titres formés, slugs, rôles.
async fn finish_jorf(repo: &DecisionRepository<'_>) -> Result<()> {
    // Titre du code dénormalisé → titre formé `search_title` (ADR 0114).
    let retitled = repo
        .refresh_article_code_titles()
        .await
        .map_err(|e| anyhow!("refresh_article_code_titles: {e}"))?;

    // Slugs des textes nouveaux (ADR 0162, unique écrivain de la colonne).
    let slugged = super::slugs::assign_text_slugs(repo).await?;

    // Rôles recalculés sur tout le référentiel (ADR 0246, dérivés/rejouables) :
    // les fiches fraîches du JO (nominations, habilitations, véhicules) sortent
    // de la recherche par défaut dès leur entrée.
    let (individuel, habilitation, vehicule) = repo
        .backfill_text_roles()
        .await
        .map_err(|e| anyhow!("backfill_text_roles: {e}"))?;

    tracing::info!(
        retitled,
        slugged,
        individuel,
        habilitation,
        vehicule,
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
    let downloaded = tokio::task::spawn_blocking(move || {
        lj_sources::dila::sync_dila(&cache_dir, lj_sources::dila::DilaFond::Jorf)
    })
    .await
    .map_err(|e| anyhow!("sync_dila join (jorf): {e}"))?
    .map_err(|e| anyhow!("sync_dila (jorf): {e}"))?;
    if downloaded.is_empty() {
        tracing::info!("sync_jorf : rien de neuf (≤ watermark)");
        return Ok(());
    }
    tracing::info!(
        downloaded = downloaded.len(),
        "sync_jorf : ingestion des archives fraîches"
    );
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Finisseurs pleine-table (1,4 M de titres) : au-delà du statement_timeout
    // de 30 s du pool.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    for path in &downloaded {
        ingest_jorf_archive(&repo, path).await?;
    }
    finish_jorf(&repo).await
}

/// Compteurs de la passe textes : traités upsertés, fiches JO créées, fiches JO
/// déjà portées (LEGI/TNC ou run antérieur — jamais réécrites), liens écrits.
#[derive(Default)]
struct JorfTextCounts {
    treaties: usize,
    created: usize,
    existing: usize,
    links: u64,
}

/// Passe 1 — textes JORF → `legal_text`, **fond complet** (ADR 0246, plan
/// phase 4) : les traités gardent l'upsert intégral (ADR 0109) ; tout le reste
/// du JO entre en fiche métadonnées (sans corps ni articles — le résidu
/// articles JO reste purgé, ADR 0115 §5) via [`insert_legal_text_if_absent`]
/// — un JORFTEXT déjà porté par LEGI n'est jamais écrasé. Les fiches créées
/// écrivent leurs liens texte-niveau (`legal_link`, graphe de généalogie).
///
/// [`insert_legal_text_if_absent`]: DecisionRepository::insert_legal_text_if_absent
async fn ingest_jorf_texts(repo: &DecisionRepository<'_>, path: &Path) -> Result<JorfTextCounts> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<JorfTexteMsg>(JORF_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if !matches!(classify_jorf_member(&name), JorfMember::Texte) {
                return Ok(());
            }
            match lj_extract::jorf::parse_jorf_texte(&raw) {
                Ok(t) => tx
                    .blocking_send(jorf_texte_msg(t))
                    .map_err(|_| anyhow!("canal JORF textes fermé")),
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "jorf texte: parse échec");
                    Ok(())
                }
            }
        })
    });

    let mut batch: Vec<JorfTexteMsg> = Vec::new();
    let mut counts = JorfTextCounts::default();
    while let Some(msg) = rx.recv().await {
        batch.push(msg);
        if batch.len() >= JORF_BATCH_SIZE {
            flush_jorf_texts(repo, std::mem::take(&mut batch), &mut counts).await?;
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture JORF textes {}: {e}", path.display()))??;
    if !batch.is_empty() {
        flush_jorf_texts(repo, batch, &mut counts).await?;
    }
    Ok(counts)
}

/// Passe 2 — articles JORF → `referential_articles`. Renvoie
/// `(upserted, skipped, no_num, errors)`.
async fn ingest_jorf_articles(
    repo: &DecisionRepository<'_>,
    path: &Path,
    treaty_cids: Arc<HashSet<String>>,
) -> Result<(usize, usize, usize, usize)> {
    enum ArtMsg {
        Row(Box<lj_store::repository::LegalArticleRow>),
        NoNum,
        Err,
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ArtMsg>(JORF_BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if !matches!(classify_jorf_member(&name), JorfMember::Article) {
                return Ok(());
            }
            let msg = match lj_extract::jorf::parse_jorf_article(&raw) {
                Ok(art) => {
                    // ADR 0115 §5 : seuls les articles de traités sont persistés.
                    if !treaty_cids.contains(&art.jorftext) {
                        return Ok(());
                    }
                    let checksum = xxhash_rust::xxh3::xxh3_64(&raw);
                    match jorf_article_row(art, checksum) {
                        Ok(Some(row)) => ArtMsg::Row(Box::new(row)),
                        Ok(None) => ArtMsg::NoNum,
                        Err(e) => {
                            tracing::error!(member = %name, error = %e, "jorf article: row invalide");
                            ArtMsg::Err
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "jorf article: parse échec");
                    ArtMsg::Err
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal JORF articles fermé"))
        })
    });

    let mut batch: Vec<lj_store::repository::LegalArticleRow> = Vec::new();
    let (mut upserted, mut skipped, mut no_num, mut errors) = (0usize, 0usize, 0usize, 0usize);
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
            ArtMsg::NoNum => no_num += 1,
            ArtMsg::Err => errors += 1,
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture JORF articles {}: {e}", path.display()))??;
    if !batch.is_empty() {
        flush_jorf_articles(repo, batch, &mut upserted, &mut skipped).await?;
    }
    Ok((upserted, skipped, no_num, errors))
}

/// Écrit un batch de textes JORF : traité → upsert intégral ; sinon insertion
/// si absent (règle d'autorité LEGI) + liens texte-niveau des fiches créées
/// (batchés par [`DecisionRepository::replace_legal_links`], rejouables —
/// remplacement par propriétaire).
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
        if msg.treaty {
            repo.upsert_legal_text(&msg.row)
                .await
                .map_err(|e| anyhow!("upsert_legal_text {}: {e}", msg.row.text_uid))?;
            counts.treaties += 1;
        } else if repo
            .insert_legal_text_if_absent(&msg.row)
            .await
            .map_err(|e| anyhow!("insert_legal_text_if_absent {}: {e}", msg.row.text_uid))?
        {
            counts.created += 1;
            if !msg.liens.is_empty() {
                let owner = lj_store::repository::LegalLinkOwner {
                    text_uid: msg.row.text_uid.clone(),
                    num_key: String::new(),
                    date_debut: msg.row.date_texte,
                };
                links.push((owner, jorf_link_rows(msg.liens)?));
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

/// Upsert d'un batch d'articles JORF (idempotent #7) ; cumule modifiés/skippés.
async fn flush_jorf_articles(
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
