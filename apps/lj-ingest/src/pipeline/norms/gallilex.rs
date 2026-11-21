//! The French Community's acts Justel does not list, from Gallilex's pages and PDFs a browser fetched into the cache.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use chrono::{NaiveDate, Utc};
use lj_store::repository::DecisionRepository;

use super::super::legal_corpus::{load_doc, run_corpus_finishers, CorpusArticle, CorpusDoc};
use super::belgium;
use crate::config::Settings;

const SOURCE: &str = "gallilex";

pub(super) async fn sync(limit: Option<usize>, fresh: bool, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("gallilex");
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let visits = match fresh {
        true => Default::default(),
        false => repo
            .norm_visits(SOURCE)
            .await
            .map_err(|e| anyhow!("norm_visits {SOURCE}: {e}"))?,
    };
    let read_on = Utc::now().date_naive();
    let mut pages: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    pages.sort();
    let (mut written, mut unchanged, mut no_article, mut no_pdf) = (0usize, 0usize, 0usize, 0usize);
    let mut publie_targets = HashSet::new();
    for path in pages {
        if limit.is_some_and(|l| written >= l) {
            break;
        }
        let page = lj_parse::gallilex_page(&std::fs::read_to_string(&path)?)
            .map_err(|e| anyhow!("{}: {e}", path.display()))?;
        let Ok(pdf) = std::fs::read(dir.join(format!("{}.pdf", page.document))) else {
            tracing::warn!(
                document = page.document,
                "gallilex page cached without its pdf"
            );
            no_pdf += 1;
            continue;
        };
        let checksum = xxhash_rust::xxh3::xxh3_64(&[format!("{page:?}").as_bytes(), &pdf].concat());
        if visits
            .get(&page.document)
            .is_some_and(|v| v.content_checksum == checksum)
        {
            unchanged += 1;
            continue;
        }
        let xml = tokio::task::spawn_blocking(move || lj_sources::pdf::pdftohtml_xml_extract(&pdf))
            .await
            .map_err(|e| anyhow!("{}: pdftohtml: {e}", page.document))??;
        let (articles, updated) = match lj_parse::gallilex_articles(&xml) {
            Ok(read) => read,
            Err(e) => {
                tracing::info!(
                    document = page.document,
                    title = page.title,
                    error = e,
                    "gallilex document shows no article"
                );
                no_article += 1;
                if !dry_run {
                    repo.put_norm_visits(SOURCE, &[(page.document.clone(), checksum)])
                        .await
                        .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))?;
                }
                continue;
            }
        };
        let adopted = NaiveDate::parse_from_str(&page.adopted, "%Y-%m-%d")?;
        let title = belgium::FRENCH_COMMUNITY.title(&page.title, Some(adopted));
        let text_uid = format!("BE-CFR/G{}", page.document);
        tracing::info!(
            text_uid,
            articles = articles.len(),
            updated,
            "gallilex document read"
        );
        if dry_run {
            written += 1;
            continue;
        }
        let doc = CorpusDoc {
            text_uid,
            source: SOURCE.to_string(),
            jurisdiction: "BE".to_string(),
            nature: belgium::nature(&title).to_string(),
            title,
            title_key: None,
            translation: "officiel".to_string(),
            source_url: Some(format!(
                "https://gallilex.cfwb.be/textes-normatifs/{}",
                page.document
            )),
            source_asof: updated,
            source_upstream_url: None,
            date_texte: Some(page.adopted.clone()),
            effect_date: None,
            detect_abrogation: true,
            num_prefix_agnostic: false,
            articles: articles
                .into_iter()
                .map(|a| CorpusArticle {
                    num: a.num,
                    num_key: None,
                    texte: Some(a.texte),
                    texte_original: None,
                    lang_original: None,
                    translation: None,
                    nota: a.nota,
                    versions: None,
                    title_path: a.title_path,
                })
                .collect(),
            links: Vec::new(),
        };
        load_doc(&repo, &doc, read_on, SOURCE, &mut publie_targets).await?;
        repo.put_norm_visits(SOURCE, &[(page.document.clone(), checksum)])
            .await
            .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))?;
        written += 1;
    }
    tracing::info!(
        publisher = SOURCE,
        written,
        unchanged,
        no_article,
        no_pdf,
        dry_run,
        "gallilex cache loaded"
    );
    if written > 0 && !dry_run {
        run_corpus_finishers(&repo, publie_targets).await?;
    }
    Ok(())
}
