//! `disposable_domains`, the mail domains no account opens on.

use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// The longest of `domains` the list carries.
    #[tracing::instrument(name = "db.listed_disposable_domain", skip(self), fields(db.system = "postgresql"))]
    pub async fn listed_disposable_domain(&self, domains: &[&str]) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT domain FROM disposable_domains WHERE domain = ANY($1) \
                 ORDER BY length(domain) DESC LIMIT 1",
                &[&domains],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Makes `domains` the list, in one statement. Returns how many domains
    /// entered and how many left.
    #[tracing::instrument(name = "db.replace_disposable_domains", skip(self, domains), fields(db.system = "postgresql", domains = domains.len()))]
    pub async fn replace_disposable_domains(&self, domains: &[String]) -> Result<(i64, i64)> {
        let row = self
            .conn
            .query_one(
                "WITH listed AS (SELECT unnest($1::text[]) AS domain), \
                 gone AS (DELETE FROM disposable_domains d \
                          WHERE NOT EXISTS (SELECT 1 FROM listed l WHERE l.domain = d.domain) \
                          RETURNING 1), \
                 came AS (INSERT INTO disposable_domains SELECT domain FROM listed \
                          ON CONFLICT DO NOTHING RETURNING 1) \
                 SELECT (SELECT count(*) FROM came), (SELECT count(*) FROM gone)",
                &[&domains],
            )
            .await?;
        Ok((row.get(0), row.get(1)))
    }
}
