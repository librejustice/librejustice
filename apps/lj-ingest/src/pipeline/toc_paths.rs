//! Audit of the section paths: every text's tree at today's date, the paths it
//! gives, and the sections that still needed an ordinal, which are defects.

use anyhow::{anyhow, Result};
use futures::StreamExt;
use lj_norm::toc_path::{toc_paths, TocKind};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const CONCURRENCY: usize = 8;
const EXAMPLES: usize = 60;
const PER_TEXT: usize = 2;

pub async fn audit_toc_paths() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_pg::db::build_pool(&settings.db_url, CONCURRENCY + 1)
        .map_err(|e| anyhow!("build_pool: {e}"))?;
    let texts = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        DecisionRepository::new(&conn)
            .toc_text_uids()
            .await
            .map_err(|e| anyhow!("toc_text_uids: {e}"))?
    };
    let today = chrono::Utc::now().date_naive();
    let mut results = futures::stream::iter(texts.iter().cloned())
        .map(|text_uid| {
            let pool = pool.clone();
            async move {
                let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
                let repo = DecisionRepository::new(&conn);
                let tree = repo
                    .toc_tree(&text_uid, today)
                    .await
                    .map_err(|e| anyhow!("toc_tree {text_uid}: {e}"))?;
                let keys = repo
                    .text_article_keys(&text_uid)
                    .await
                    .map_err(|e| anyhow!("text_article_keys {text_uid}: {e}"))?;
                let rows: Vec<_> = tree.iter().map(|r| r.toc_row()).collect();
                let paths = toc_paths(&rows, &keys.iter().map(String::as_str).collect());
                let longest = paths
                    .nodes
                    .iter()
                    .filter_map(|n| n.path.as_deref())
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let sections = paths
                    .nodes
                    .iter()
                    .filter(|n| n.kind == TocKind::Section)
                    .count();
                let addressed = paths.nodes.iter().filter(|n| n.path.is_some()).count();
                let merged = paths
                    .nodes
                    .iter()
                    .filter(|n| n.kind == TocKind::Section && n.rows.len() > 1)
                    .count();
                let ordinals: Vec<String> = paths
                    .nodes
                    .iter()
                    .filter(|n| n.ordinal)
                    .take(PER_TEXT)
                    .map(|n| {
                        let label = &tree[n.rows[0]].label;
                        format!("{text_uid} {} {label}", n.path.as_deref().unwrap_or(""))
                    })
                    .collect();
                let ordinal_count = paths.nodes.iter().filter(|n| n.ordinal).count();
                Ok::<_, anyhow::Error>((
                    sections,
                    addressed,
                    merged,
                    ordinal_count,
                    ordinals,
                    longest,
                ))
            }
        })
        .buffer_unordered(CONCURRENCY);
    let (mut sections, mut addressed, mut merged, mut ordinals, mut done) = (0, 0, 0, 0, 0);
    let (mut longest, mut over_200) = (0, 0);
    let mut examples: Vec<String> = Vec::new();
    while let Some(r) = results.next().await {
        let (s, a, m, n, o, l) = r?;
        sections += s;
        addressed += a;
        merged += m;
        ordinals += n;
        longest = longest.max(l);
        over_200 += usize::from(l > 200);
        examples.extend(o.into_iter().take(EXAMPLES.saturating_sub(examples.len())));
        done += 1;
    }
    for e in &examples {
        tracing::warn!(example = %e, "section path needed an ordinal");
    }
    tracing::info!(
        texts = done,
        sections,
        addressed,
        merged,
        ordinals,
        longest,
        texts_over_200 = over_200,
        "section paths audited"
    );
    Ok(())
}
