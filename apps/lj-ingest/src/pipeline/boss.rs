//! The BOSS: each rubric a `legal_text` `BOSS-<rubric>`, each numbered paragraph a `legal_article`.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_sources::bofip::Paragraph;
use lj_sources::boss::{self, Version};
use lj_sources::bulletins::BulletinClient;
use lj_store::repository::{
    BodyWrite, DecisionRepository, Fraicheur, LegalArticleRow, LegalTextRow,
};

use super::corpus_toc::{derive_corpus_toc, TocArticle, TocVersion};
use crate::config::Settings;

const SOURCE: &str = "boss";

struct Rubric {
    uid: String,
    title: String,
    url: String,
    /// The version in force first, then the earlier ones, newest first.
    versions: Vec<(Version, Vec<Paragraph>)>,
    /// The text in force of a rubric with no numbered paragraph.
    body: Option<String>,
}

/// Every rubric and each of its versions the site offers; an earlier version, which
/// never changes, is read once and kept in the cache.
pub async fn sync_boss(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = BulletinClient::new(
        (!settings.page_gateway.is_empty()).then(|| settings.page_gateway.clone()),
        (!settings.egress_proxy.is_empty()).then(|| settings.egress_proxy.clone()),
        settings.cache_dir(),
    )?;
    let versions_dir = settings.cache_dir().join("boss").join("versions");
    tokio::fs::create_dir_all(&versions_dir).await?;
    let paths = boss::rubrics(&client)
        .await
        .map_err(|e| anyhow!("boss rubrics: {e}"))?;
    let mut rubrics = Vec::new();
    for path in &paths {
        let url = boss::rubric_url(path);
        match read_rubric(&client, &versions_dir, &url).await {
            Ok((title, versions, body)) => rubrics.push(Rubric {
                uid: format!("BOSS-{}", path.rsplit('/').next().unwrap_or(path)),
                title,
                url,
                versions,
                body,
            }),
            Err(e) => {
                tracing::warn!(rubric = %path, error = %e, "boss: rubric failed, read again next run")
            }
        }
    }
    if rubrics.is_empty() {
        anyhow::bail!("boss: none of {} rubrics answered", paths.len());
    }
    for r in &rubrics {
        tracing::info!(
            uid = %r.uid,
            title = %r.title,
            versions = r.versions.len(),
            paragraphs = r.versions[0].1.len(),
            rows = article_versions(&r.versions).len(),
            body_chars = r.body.as_ref().map_or(0, String::len),
            "boss rubric"
        );
    }
    if dry_run {
        return Ok(());
    }

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let today = chrono::Utc::now().date_naive();
    let mut rows = 0;
    for r in &rubrics {
        rows += write(&repo, r, today).await?;
    }
    let (ranked_texts, ranked_rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(
        rubrics = rubrics.len(),
        failed = paths.len() - rubrics.len(),
        rows,
        ranked_texts,
        ranked_rows,
        slugged,
        "sync_boss"
    );
    Ok(())
}

async fn read_rubric(
    client: &BulletinClient,
    versions_dir: &std::path::Path,
    url: &str,
) -> Result<(String, Vec<(Version, Vec<Paragraph>)>, Option<String>)> {
    let rubric = boss::rubric(client, url).await?;
    if let Some(body) = rubric.body {
        return Ok((
            rubric.title,
            vec![(rubric.versions[0].clone(), Vec::new())],
            Some(body),
        ));
    }
    if rubric.paragraphs.is_empty() {
        anyhow::bail!("boss: rubric without text");
    }
    let mut versions = vec![(rubric.versions[0].clone(), rubric.paragraphs)];
    for v in &rubric.versions[1..] {
        let fragment = v
            .url
            .as_deref()
            .expect("an earlier version has its fragment");
        let cached = versions_dir.join(format!(
            "{:016x}.html",
            xxhash_rust::xxh3::xxh3_64(fragment.as_bytes())
        ));
        let html = match tokio::fs::read_to_string(&cached).await {
            Ok(html) => html,
            Err(_) => {
                let html = boss::fragment(client, fragment).await?;
                tokio::fs::write(&cached, &html).await?;
                html
            }
        };
        versions.push((v.clone(), boss::parse_version(&html)));
    }
    Ok((rubric.title, versions, None))
}

#[derive(Debug, PartialEq, Eq)]
struct ArticleVersion {
    num: String,
    from: NaiveDate,
    /// The first day the paragraph no longer reads so.
    until: Option<NaiveDate>,
    texte: String,
    section_path: Vec<String>,
    /// A later row holds the same paragraph.
    superseded: bool,
}

/// A paragraph keeps one row while its text stays the same across consecutive versions.
fn article_versions(versions: &[(Version, Vec<Paragraph>)]) -> Vec<ArticleVersion> {
    let mut rows: Vec<ArticleVersion> = Vec::new();
    let mut latest: BTreeMap<&str, usize> = BTreeMap::new();
    for (v, paragraphs) in versions.iter().rev() {
        for p in paragraphs {
            match latest.get(p.num.as_str()) {
                Some(&i) if rows[i].texte == p.texte && rows[i].until == Some(v.from) => {
                    rows[i].until = v.until;
                    rows[i].section_path.clone_from(&p.section_path);
                }
                found => {
                    if let Some(&i) = found {
                        rows[i].superseded = true;
                    }
                    latest.insert(&p.num, rows.len());
                    rows.push(ArticleVersion {
                        num: p.num.clone(),
                        from: v.from,
                        until: v.until,
                        texte: p.texte.clone(),
                        section_path: p.section_path.clone(),
                        superseded: false,
                    });
                }
            }
        }
    }
    rows
}

/// The end date and the status enter the checksum: a paragraph that leaves the
/// rubric changes them alone.
async fn write(repo: &DecisionRepository<'_>, r: &Rubric, today: NaiveDate) -> Result<usize> {
    let uid = r.uid.as_str();
    repo.upsert_legal_text(&LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: "FR".to_string(),
        title: r.title.clone(),
        title_key: lj_extract::extract::normalize_instrument(&r.title),
        nature: "BOSS".to_string(),
        last_modified: Some(r.versions[0].0.from),
        date_texte: None,
        date_publi: r.versions.last().map(|(v, _)| v.from),
        eli: None,
        nor: None,
        instrument_key: None,
        // The site keeps an abrogated instruction under « [ Instruction du 5 mars 2021 abrogée ] ».
        status: Some(
            if r.title.starts_with('[') && r.title.contains("abrogée") {
                "ABROGE"
            } else {
                "VIGUEUR"
            }
            .to_string(),
        ),
    })
    .await
    .map_err(|e| anyhow!("upsert_legal_text {uid}: {e}"))?;
    if let Some(body) = &r.body {
        repo.write_text_bodies(
            SOURCE,
            &[(uid.to_string(), body.clone())],
            BodyWrite::Replace,
        )
        .await
        .map_err(|e| anyhow!("write_text_bodies {uid}: {e}"))?;
        return Ok(1);
    }
    let position: BTreeMap<&str, i32> = r.versions[0]
        .1
        .iter()
        .enumerate()
        .map(|(i, p)| (p.num.as_str(), i as i32))
        .collect();
    let rows = article_versions(&r.versions);
    let mut toc: BTreeMap<&str, TocArticle> = BTreeMap::new();
    for a in &rows {
        let num_key = lj_norm::article_key::identity_key(&a.num);
        let status = match (a.until, a.superseded) {
            (None, _) => "VIGUEUR",
            (Some(_), true) => "MODIFIE",
            (Some(_), false) => "ABROGE",
        };
        let date_fin = a.until.map(|d| d - chrono::Days::new(1));
        let source_uid = format!("{uid}#{}@{}", a.num, a.from);
        let entry = toc.entry(&a.num).or_insert_with(|| TocArticle {
            num: a.num.clone(),
            num_key: num_key.clone(),
            versions: Vec::new(),
            title_path: None,
        });
        entry.versions.push(TocVersion {
            source_uid: source_uid.clone(),
            status: status.to_string(),
            date_debut: Some(a.from),
            date_fin,
        });
        if !a.section_path.is_empty() {
            entry.title_path = Some(a.section_path.join(" > "));
        }
        let checksum =
            xxhash_rust::xxh3::xxh3_64(format!("{status}\n{date_fin:?}\n{}", a.texte).as_bytes());
        repo.upsert_legal_article(
            &LegalArticleRow {
                text_uid: uid.to_string(),
                num: a.num.clone(),
                num_key,
                position: position.get(a.num.as_str()).copied(),
                title_path: Some(r.title.clone()),
                status: status.to_string(),
                date_debut: Some(a.from),
                date_fin,
                texte: (!a.texte.is_empty()).then(|| a.texte.clone()),
                texte_original: None,
                lang_original: None,
                translation: "officiel".to_string(),
                nota: None,
                content_checksum: checksum,
                source: SOURCE.to_string(),
                source_uid,
                source_url: Some(format!("{}#{}", r.url, a.num)),
                source_asof: Some(today),
                source_upstream_url: None,
            },
            Fraicheur::Autoritaire,
        )
        .await
        .map_err(|e| anyhow!("upsert_legal_article {uid} § {}: {e}", a.num))?;
    }
    repo.delete_toc_edges_by_text(uid)
        .await
        .map_err(|e| anyhow!("delete_toc_edges_by_text {uid}: {e}"))?;
    let mut articles: Vec<TocArticle> = r.versions[0]
        .1
        .iter()
        .filter_map(|p| toc.remove(p.num.as_str()))
        .collect();
    articles.extend(toc.into_values());
    repo.replace_toc_edges(&derive_corpus_toc(uid, &articles))
        .await
        .map_err(|e| anyhow!("replace_toc_edges {uid}: {e}"))?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn p(num: &str, texte: &str) -> Paragraph {
        Paragraph {
            num: num.to_string(),
            texte: texte.to_string(),
            section_path: vec!["Chapitre 1".to_string()],
        }
    }

    fn v(from: &str, until: Option<&str>) -> Version {
        Version {
            from: d(from),
            until: until.map(d),
            url: until.map(|_| "fragment".to_string()),
        }
    }

    #[test]
    fn a_paragraph_keeps_its_row_until_its_text_changes_or_it_leaves() {
        let versions = vec![
            (v("2026-06-01", None), vec![p("10", "A"), p("20", "B2")]),
            (
                v("2026-01-01", Some("2026-06-01")),
                vec![p("10", "A"), p("20", "B"), p("30", "C")],
            ),
            (
                v("2025-01-01", Some("2026-01-01")),
                vec![p("10", "A"), p("20", "B"), p("30", "C")],
            ),
        ];
        let rows = article_versions(&versions);
        let at = |num: &str, from: &str| {
            rows.iter()
                .find(|r| r.num == num && r.from == d(from))
                .map(|r| (r.until, r.superseded))
        };
        assert_eq!(rows.len(), 4);
        assert_eq!(at("10", "2025-01-01"), Some((None, false)));
        assert_eq!(at("20", "2025-01-01"), Some((Some(d("2026-06-01")), true)));
        assert_eq!(at("20", "2026-06-01"), Some((None, false)));
        assert_eq!(at("30", "2025-01-01"), Some((Some(d("2026-06-01")), false)));
    }
}
