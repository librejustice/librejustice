//! Jurisdiction hubs: the aggregated catalogue, the years a jurisdiction
//! covers, the page of a year's decisions.
//!
//! The three aggregate reads live on `jurisdiction_year_counts`, never on
//! `decisions`, which the crawl asks for over every hub page. The listing
//! itself stays on `decisions`.

use super::types::{HubDecisionRow, JurisdictionHubPages, JurisdictionHubRow};
use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// Catalogue des juridictions avec volume de décisions actives
    /// (`/decision-index/{juris}`). Le total somme **toutes** les années du code, ligne
    /// `year IS NULL` comprise : les décisions sans date comptent ici, et pas
    /// dans l'histogramme.
    #[tracing::instrument(name = "db.jurisdiction_catalogue", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_catalogue(&self) -> Result<Vec<JurisdictionHubRow>> {
        let rows = self
            .conn
            .query(
                "SELECT c.code, j.jurisdiction_type, j.label, c.n \
                 FROM (SELECT coalesce(o.successor_code, o.code) code, \
                              sum(y.decision_count)::bigint n \
                       FROM jurisdiction_year_counts y \
                       JOIN jurisdiction o ON o.code = y.jurisdiction_code \
                       GROUP BY 1) c \
                 JOIN jurisdiction j ON j.code = c.code \
                 ORDER BY j.jurisdiction_type, j.label",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| JurisdictionHubRow {
                code: r.get(0),
                jurisdiction_type: r.get(1),
                label: r.get(2),
                decision_count: r.get(3),
            })
            .collect())
    }

    /// Années couvertes par une juridiction, avec compteur, plus récente en
    /// tête (`/decision-index/{juris}/{code}`). Vide si le code est inconnu.
    #[tracing::instrument(name = "db.jurisdiction_years", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_years(&self, code: &str) -> Result<Vec<(i32, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT year, sum(decision_count)::bigint \
                 FROM jurisdiction_year_counts \
                 WHERE jurisdiction_code IN (SELECT code FROM jurisdiction \
                                             WHERE (code = $1 AND successor_code IS NULL) \
                                                OR successor_code = $1) \
                   AND year IS NOT NULL \
                 GROUP BY year ORDER BY year DESC",
                &[&code],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Total d'un couple juridiction×année (`/decision-index/{juris}/{code}/{annee}`).
    /// `None` quand le couple est vide.
    #[tracing::instrument(name = "db.jurisdiction_year_total", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_year_total(&self, code: &str, year: i32) -> Result<Option<i64>> {
        let row = self
            .conn
            .query_opt(
                "SELECT sum(decision_count)::bigint FROM jurisdiction_year_counts \
                 WHERE jurisdiction_code IN (SELECT code FROM jurisdiction \
                                             WHERE (code = $1 AND successor_code IS NULL) \
                                                OR successor_code = $1) \
                   AND year = $2 \
                 HAVING count(*) > 0",
                &[&code, &year],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Triplets `(type, code, [(année, nombre de pages)])` pour la section
    /// `juridictions` des sitemaps, années récentes en tête. `page_size` est
    /// celui que sert l'API : au-delà du compte, la page rend un listing vide
    /// en 200.
    #[tracing::instrument(name = "db.iter_jurisdiction_hubs_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_jurisdiction_hubs_for_sitemap(
        &self,
        page_size: i64,
    ) -> Result<JurisdictionHubPages> {
        // `$1::bigint` avant le float8 : sous `$1::float8` seul, Postgres infère
        // le paramètre en float8 et refuse d'y sérialiser l'i64.
        let rows = self
            .conn
            .query(
                "SELECT j.jurisdiction_type, c.code, c.year, \
                        ceil(c.n::float8 / $1::bigint::float8)::bigint \
                 FROM (SELECT coalesce(o.successor_code, o.code) code, y.year, \
                              sum(y.decision_count) n \
                       FROM jurisdiction_year_counts y \
                       JOIN jurisdiction o ON o.code = y.jurisdiction_code \
                       WHERE y.year IS NOT NULL \
                       GROUP BY 1, 2) c \
                 JOIN jurisdiction j ON j.code = c.code \
                 ORDER BY 2, 3 DESC",
                &[&page_size],
            )
            .await?;
        let mut out: JurisdictionHubPages = Vec::new();
        for r in rows {
            let code: String = r.get(1);
            let year = (r.get(2), r.get(3));
            match out.last_mut() {
                Some((_, c, years)) if *c == code => years.push(year),
                _ => out.push((r.get(0), code, vec![year])),
            }
        }
        Ok(out)
    }

    /// Rebuild intégral de `jurisdiction_year_counts`. Une seule instruction
    /// (CTE modifiantes, même snapshot) : upsert des couples recomptés,
    /// suppression des disparus, garde `<>` pour ne réécrire que les décomptes
    /// changés. Renvoie le nombre de couples upsertés.
    #[tracing::instrument(name = "db.rebuild_jurisdiction_year_counts", skip(self), fields(db.system = "postgresql"))]
    pub async fn rebuild_jurisdiction_year_counts(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                WITH fresh AS (
                    SELECT jurisdiction_code,
                           extract(year FROM date_lecture)::int AS year,
                           count(*) AS decision_count
                    FROM decisions
                    WHERE deleted_at IS NULL AND kind = 'decision'
                    GROUP BY 1, 2
                ), stale AS (
                    DELETE FROM jurisdiction_year_counts c
                    WHERE NOT EXISTS (
                        SELECT 1 FROM fresh f
                        WHERE f.jurisdiction_code = c.jurisdiction_code
                          AND f.year IS NOT DISTINCT FROM c.year)
                )
                INSERT INTO jurisdiction_year_counts
                            (jurisdiction_code, year, decision_count)
                SELECT jurisdiction_code, year, decision_count FROM fresh
                ON CONFLICT (jurisdiction_code, year) DO UPDATE
                SET decision_count = excluded.decision_count
                WHERE jurisdiction_year_counts.decision_count
                      <> excluded.decision_count
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Page de décisions d'une juridiction×année, date décroissante
    /// (`/decision-index/{juris}/{code}/{annee}?page=N`).
    #[tracing::instrument(name = "db.jurisdiction_year_decisions", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_year_decisions(
        &self,
        code: &str,
        year: i32,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<HubDecisionRow>> {
        // The `date_iso` alias is what lets `ORDER BY date_lecture` name the
        // TABLE column: without it the name binds to the output column, the text,
        // and the index no longer gives the order (a Bitmap Heap Scan and a
        // parallel sort instead of a lone Index Scan).
        // A court and the closed ones it succeeded rarely share a year: the
        // year that has one code keeps the index order.
        let codes: Vec<String> = self
            .conn
            .query(
                "SELECT jurisdiction_code FROM jurisdiction_year_counts \
                 WHERE jurisdiction_code IN (SELECT code FROM jurisdiction \
                                             WHERE (code = $1 AND successor_code IS NULL) \
                                                OR successor_code = $1) \
                   AND year = $2",
                &[&code, &year],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let rows = match codes.as_slice() {
            [] => Vec::new(),
            [one] => {
                self.conn
                    .query(
                        "SELECT public_id, search_title, date_lecture::text AS date_iso \
                         FROM decisions \
                         WHERE jurisdiction_code = $1 \
                           AND date_lecture >= make_date($2, 1, 1) \
                           AND date_lecture < make_date($2 + 1, 1, 1) \
                           AND deleted_at IS NULL AND kind = 'decision' \
                         ORDER BY date_lecture DESC, id \
                         LIMIT $3 OFFSET $4",
                        &[one, &year, &limit, &offset],
                    )
                    .await?
            }
            several => {
                self.conn
                    .query(
                        "SELECT public_id, search_title, date_lecture::text AS date_iso \
                         FROM decisions \
                         WHERE jurisdiction_code = ANY($1) \
                           AND date_lecture >= make_date($2, 1, 1) \
                           AND date_lecture < make_date($2 + 1, 1, 1) \
                           AND deleted_at IS NULL AND kind = 'decision' \
                         ORDER BY date_lecture DESC, id \
                         LIMIT $3 OFFSET $4",
                        &[&several, &year, &limit, &offset],
                    )
                    .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| HubDecisionRow {
                public_id: r.get(0),
                title: r.get(1),
                date_lecture: r.get(2),
            })
            .collect())
    }
}
