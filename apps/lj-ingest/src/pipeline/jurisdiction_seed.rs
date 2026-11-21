//! The courts the referential holds by their own name: closed ones with their
//! closing date and successor, the successors no decision has created yet, and
//! the criminal trial courts of every département.

use anyhow::{anyhow, Result};
use lj_store::repository::{DecisionRepository, SeededJurisdiction};

use crate::config::Settings;

pub(super) fn seed() -> Vec<SeededJurisdiction> {
    include_str!("../../data/jurisdiction_seed.tsv")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            let [code, jurisdiction_type, city, label, closed_on, successor_code] = cols[..] else {
                panic!("jurisdiction_seed.tsv: malformed line {l:?}");
            };
            let filled = |s: &str| (!s.is_empty()).then(|| s.to_string());
            SeededJurisdiction {
                code: code.to_string(),
                jurisdiction_type: jurisdiction_type.to_string(),
                city: city.to_string(),
                label: label.to_string(),
                closed_on: filled(closed_on).map(|d| {
                    d.parse()
                        .unwrap_or_else(|_| panic!("jurisdiction_seed.tsv: date {d:?}"))
                }),
                successor_code: filled(successor_code),
            }
        })
        .chain(lj_extract::facets::criminal_courts().into_iter().map(|c| {
            SeededJurisdiction {
                code: c.code,
                jurisdiction_type: c.jurisdiction_type,
                city: c
                    .city
                    .expect("a criminal trial court sits in a département"),
                label: c.label,
                closed_on: None,
                successor_code: None,
            }
        }))
        .collect()
}

pub async fn seed_jurisdictions() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let rows = seed();
    let written = DecisionRepository::new(&conn)
        .seed_jurisdictions(&rows)
        .await?;
    tracing::info!(rows = rows.len(), written, "jurisdiction seed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seat files a court under its successor in one step, so a successor
    /// is never itself filed under another, and a closed court names one.
    #[test]
    fn the_seed_reads_and_a_successor_sits() {
        let rows = seed();
        assert!(rows.len() > 200);
        let filed: std::collections::HashSet<&str> = rows
            .iter()
            .filter(|r| r.successor_code.is_some())
            .map(|r| r.code.as_str())
            .collect();
        for r in &rows {
            if let Some(s) = &r.successor_code {
                assert!(!filed.contains(s.as_str()), "{} under {s}", r.code);
            }
            assert!(
                r.closed_on.is_none() || r.successor_code.is_some(),
                "{}",
                r.code
            );
        }
    }
}
