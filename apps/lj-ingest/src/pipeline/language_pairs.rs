//! Decisions a source holds once per language, merged into one carrying each language's text.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// `pairs` holds a `KEEPER<TAB>ABSORBED` pair of public ids a line, `#` lines left out; one pair
/// not live at one day and one court, or a court and its successor, and nothing is merged.
pub async fn merge_language_pairs(pairs: &Path, apply: bool) -> Result<()> {
    let list =
        std::fs::read_to_string(pairs).with_context(|| format!("read {}", pairs.display()))?;
    let pairs: Vec<(&str, &str)> = list
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut cols = l.split('\t');
            match (cols.next(), cols.next()) {
                (Some(keeper), Some(copy)) => Ok((keeper.trim(), copy.trim())),
                _ => Err(anyhow!("`{l}`: KEEPER<TAB>ABSORBED expected")),
            }
        })
        .collect::<Result<_>>()?;
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let successors: HashMap<String, String> =
        repo.jurisdiction_successors().await?.into_iter().collect();
    let one_court = |a: &Option<String>, b: &Option<String>| {
        a == b
            || a.as_ref().is_some_and(|a| successors.get(a) == b.as_ref())
            || b.as_ref().is_some_and(|b| successors.get(b) == a.as_ref())
    };
    let mut checked = Vec::with_capacity(pairs.len());
    let mut refused = 0usize;
    for (keeper, copy) in &pairs {
        match (
            repo.decision_address(keeper).await?,
            repo.decision_address(copy).await?,
        ) {
            (Some((k, k_court, k_date)), Some((c, c_court, c_date)))
                if k != c && k_date == c_date && one_court(&k_court, &c_court) =>
            {
                checked.push((k, c));
            }
            (k, c) => {
                tracing::warn!(
                    keeper,
                    copy,
                    ?k,
                    ?c,
                    "merge-language-pairs: not one decision's address"
                );
                refused += 1;
            }
        }
    }
    tracing::warn!(
        pairs = checked.len(),
        refused,
        apply,
        "merge-language-pairs: checked"
    );
    if refused > 0 {
        bail!("{refused} pairs refused, nothing merged");
    }
    if !apply {
        return Ok(());
    }
    let mut keepers = Vec::with_capacity(checked.len());
    let mut texts = 0;
    for (keeper, copy) in checked {
        texts += repo.absorb_language_into(copy, keeper).await?;
        keepers.push(keeper);
    }
    keepers.sort_unstable();
    keepers.dedup();
    for chunk in keepers.chunks(500) {
        super::reextract::fuse_written(&repo, chunk).await?;
    }
    tracing::warn!(
        merged = keepers.len(),
        texts_moved = texts,
        "merge-language-pairs: applied"
    );
    Ok(())
}
