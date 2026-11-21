//! Hubs of the norms catalogue, `/norm-index/{juris}/{fond}/{year}`: a text's fond is
//! its first type level read by `lj_norm::norm_fond`, its browsing date `text_date`.

use std::collections::{BTreeMap, HashMap};

use super::types::{NormHubPages, NormTextRow};
use super::DecisionRepository;
use crate::error::Result;

/// A text's first type level, `FR.LOI` in `{FR, FR.LOI, FR.LOI.ORGANIQUE}`.
const FIRST_TYPE: &str = "jurisdiction_paths[2]";

/// Mirror of the index's partial predicate.
pub(super) const NORM_SCOPE: &str = "slug IS NOT NULL";

/// The texts of a fond of `$1`, the fond's first types bound at `$2`.
fn fond_filter(jurisdiction: &str, fond: &str) -> (String, Vec<String>) {
    let (paths, untyped) = lj_norm::norm_fond::fond_paths(jurisdiction, fond);
    match untyped {
        // An untyped text's second path is its territory when it has one.
        true => (
            format!("jurisdiction = $1 AND NOT coalesce({FIRST_TYPE} = ANY($2), false)"),
            lj_norm::norm_fond::other_fond_paths(jurisdiction, fond),
        ),
        false => (
            format!("jurisdiction = $1 AND {FIRST_TYPE} = ANY($2)"),
            paths,
        ),
    }
}

impl DecisionRepository<'_> {
    /// Volumes per `(jurisdiction, fond)`, behind `lj-api`'s 12 h cache.
    #[tracing::instrument(name = "db.norm_catalogue", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_catalogue(&self) -> Result<Vec<(String, String, i64)>> {
        let sql = format!(
            "SELECT jurisdiction, {FIRST_TYPE}, count(*) FROM legal_text \
             WHERE {NORM_SCOPE} GROUP BY 1, 2"
        );
        let rows = self.conn.query(&sql, &[]).await?;
        let mut counts: HashMap<(String, &'static str), i64> = HashMap::new();
        for r in &rows {
            let jurisdiction: String = r.get(0);
            let first_type: Option<String> = r.get(1);
            let fond = lj_norm::norm_fond::fond(&jurisdiction, first_type.as_deref());
            *counts.entry((jurisdiction, fond)).or_default() += r.get::<_, i64>(2);
        }
        Ok(counts
            .into_iter()
            .map(|((jurisdiction, fond), n)| (jurisdiction, fond.to_string(), n))
            .collect())
    }

    /// The years a fond covers, newest first, the undated texts last as `None`.
    #[tracing::instrument(name = "db.norm_fond_years", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_years(
        &self,
        jurisdiction: &str,
        fond: &str,
    ) -> Result<Vec<(Option<i32>, i64)>> {
        let (filter, paths) = fond_filter(jurisdiction, fond);
        let sql = format!(
            "SELECT extract(year FROM text_date)::int, count(*) \
             FROM legal_text WHERE {NORM_SCOPE} AND {filter} \
             GROUP BY 1 ORDER BY 1 DESC NULLS LAST"
        );
        let rows = self.conn.query(&sql, &[&jurisdiction, &paths]).await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// How many texts a fond has in one year, or undated with `None`.
    #[tracing::instrument(name = "db.norm_fond_year_total", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_year_total(
        &self,
        jurisdiction: &str,
        fond: &str,
        year: Option<i32>,
    ) -> Result<i64> {
        let (filter, paths) = fond_filter(jurisdiction, fond);
        let row = match year {
            Some(year) => {
                let sql = format!(
                    "SELECT count(*) FROM legal_text WHERE {NORM_SCOPE} AND {filter} \
                       AND text_date >= make_date($3, 1, 1) \
                       AND text_date < make_date($3 + 1, 1, 1)"
                );
                self.conn
                    .query_one(&sql, &[&jurisdiction, &paths, &year])
                    .await?
            }
            None => {
                let sql = format!(
                    "SELECT count(*) FROM legal_text WHERE {NORM_SCOPE} AND {filter} \
                       AND text_date IS NULL"
                );
                self.conn.query_one(&sql, &[&jurisdiction, &paths]).await?
            }
        };
        Ok(row.get(0))
    }

    /// A page of a fond's texts of one year, newest first, or of its undated ones by id.
    #[tracing::instrument(name = "db.norm_fond_year_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_year_texts(
        &self,
        jurisdiction: &str,
        fond: &str,
        year: Option<i32>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<NormTextRow>> {
        let (filter, paths) = fond_filter(jurisdiction, fond);
        let rows = match year {
            Some(year) => {
                let sql = format!(
                    "SELECT slug, title, text_date::text AS browse_date, jurisdiction \
                     FROM legal_text WHERE {NORM_SCOPE} AND {filter} \
                       AND text_date >= make_date($3, 1, 1) \
                       AND text_date < make_date($3 + 1, 1, 1) \
                     ORDER BY text_date DESC, id LIMIT $4 OFFSET $5"
                );
                self.conn
                    .query(&sql, &[&jurisdiction, &paths, &year, &limit, &offset])
                    .await?
            }
            None => {
                let sql = format!(
                    "SELECT slug, title, NULL::text, jurisdiction \
                     FROM legal_text WHERE {NORM_SCOPE} AND {filter} \
                       AND text_date IS NULL \
                     ORDER BY id LIMIT $3 OFFSET $4"
                );
                self.conn
                    .query(&sql, &[&jurisdiction, &paths, &limit, &offset])
                    .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| NormTextRow {
                slug: r.get(0),
                title: r.get(1),
                jurisdiction: r.get(3),
                date: r.get(2),
            })
            .collect())
    }

    /// A text's fond and browsing year, for the link back from its page.
    #[tracing::instrument(name = "db.norm_fond_of_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_of_text(&self, text_uid: &str) -> Result<Option<(String, Option<i32>)>> {
        let sql = format!(
            "SELECT jurisdiction, {FIRST_TYPE}, extract(year FROM text_date)::int \
             FROM legal_text WHERE text_uid = $1 AND {NORM_SCOPE}"
        );
        let rows = self.conn.query(&sql, &[&text_uid]).await?;
        Ok(rows.first().map(|r| {
            let jurisdiction: String = r.get(0);
            let first_type: Option<String> = r.get(1);
            let fond = lj_norm::norm_fond::fond(&jurisdiction, first_type.as_deref());
            (fond.to_string(), r.get(2))
        }))
    }

    /// `(jurisdiction, fond, [(year token, page count)])` for the sitemaps, newest
    /// year first; `codes`, which has no year hub, is left out.
    #[tracing::instrument(name = "db.iter_norm_hubs_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_norm_hubs_for_sitemap(&self, page_size: i64) -> Result<NormHubPages> {
        let sql = format!(
            "SELECT jurisdiction, {FIRST_TYPE}, extract(year FROM text_date)::int, count(*) \
             FROM legal_text WHERE {NORM_SCOPE} GROUP BY 1, 2, 3"
        );
        let rows = self.conn.query(&sql, &[]).await?;
        let mut counts: BTreeMap<(String, &'static str), BTreeMap<Option<i32>, i64>> =
            BTreeMap::new();
        for r in &rows {
            let jurisdiction: String = r.get(0);
            let first_type: Option<String> = r.get(1);
            let fond = lj_norm::norm_fond::fond(&jurisdiction, first_type.as_deref());
            if fond == "codes" {
                continue;
            }
            *counts
                .entry((jurisdiction, fond))
                .or_default()
                .entry(r.get(2))
                .or_default() += r.get::<_, i64>(3);
        }
        Ok(counts
            .into_iter()
            .map(|((jurisdiction, fond), years)| {
                let mut years: Vec<(Option<i32>, i64)> = years.into_iter().collect();
                years.sort_by_key(|(year, _)| (year.is_none(), std::cmp::Reverse(*year)));
                let tokens = years
                    .into_iter()
                    .map(|(year, n)| {
                        let token = year.map_or_else(|| "sans-date".to_string(), |y| y.to_string());
                        (token, (n + page_size - 1) / page_size)
                    })
                    .collect();
                (jurisdiction, fond.to_string(), tokens)
            })
            .collect())
    }
}
