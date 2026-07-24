//! Hubs juridiction (ADR 0253) : catalogue agrégé, années couvertes par une
//! juridiction, page de décisions d'une année — les chemins de crawl SSR
//! `/juridictions` → `/juridiction/{code}` → `/juridiction/{code}/{annee}`.

use super::types::{HubDecisionRow, JurisdictionHubRow};
use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// Catalogue des juridictions avec volume de décisions actives
    /// (`/juridictions`). Agrégat sur l'index `(jurisdiction_code,
    /// date_lecture)` (migration 0164) ; servi derrière le cache référentiel
    /// 12 h de `lj-api` — le seq du GROUP BY reste ponctuel.
    #[tracing::instrument(name = "db.jurisdiction_catalogue", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_catalogue(&self) -> Result<Vec<JurisdictionHubRow>> {
        let rows = self
            .conn
            .query(
                "SELECT d.jurisdiction_code, j.jurisdiction_type, j.label, count(*) \
                 FROM decisions d \
                 JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                 WHERE d.deleted_at IS NULL AND d.public_id IS NOT NULL \
                 GROUP BY 1, 2, 3 \
                 ORDER BY 2, 3",
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
    /// tête (`/juridiction/{code}`). Vide si le code est inconnu — le route
    /// handler en fait un 404.
    #[tracing::instrument(name = "db.jurisdiction_years", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_years(&self, code: &str) -> Result<Vec<(i32, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT extract(year FROM date_lecture)::int, count(*) \
                 FROM decisions \
                 WHERE jurisdiction_code = $1 AND date_lecture IS NOT NULL \
                   AND deleted_at IS NULL AND public_id IS NOT NULL \
                 GROUP BY 1 ORDER BY 1 DESC",
                &[&code],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Couples `(code, années)` pour la section `juridictions` des sitemaps
    /// (ADR 0253) : un hub par code + une URL par année, années récentes en
    /// tête. Le join sur `jurisdiction` garantit que chaque URL émise a une
    /// page (le hub 404 sur un code hors référentiel).
    #[tracing::instrument(name = "db.iter_jurisdiction_hubs_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<Vec<(String, Vec<i32>)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.jurisdiction_code, extract(year FROM d.date_lecture)::int \
                 FROM decisions d \
                 JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                 WHERE d.date_lecture IS NOT NULL \
                   AND d.deleted_at IS NULL AND d.public_id IS NOT NULL \
                 GROUP BY 1, 2 ORDER BY 1, 2 DESC",
                &[],
            )
            .await?;
        let mut out: Vec<(String, Vec<i32>)> = Vec::new();
        for r in rows {
            let code: String = r.get(0);
            let year: i32 = r.get(1);
            match out.last_mut() {
                Some((c, years)) if *c == code => years.push(year),
                _ => out.push((code, vec![year])),
            }
        }
        Ok(out)
    }

    /// Page de décisions d'une juridiction×année, date décroissante
    /// (`/juridiction/{code}/{annee}?page=N`).
    #[tracing::instrument(name = "db.jurisdiction_year_decisions", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_year_decisions(
        &self,
        code: &str,
        year: i32,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<HubDecisionRow>> {
        let rows = self
            .conn
            .query(
                "SELECT public_id, search_title, date_lecture::text \
                 FROM decisions \
                 WHERE jurisdiction_code = $1 \
                   AND date_lecture >= make_date($2, 1, 1) \
                   AND date_lecture < make_date($2 + 1, 1, 1) \
                   AND deleted_at IS NULL AND public_id IS NOT NULL \
                 ORDER BY date_lecture DESC, id \
                 LIMIT $3 OFFSET $4",
                &[&code, &year, &limit, &offset],
            )
            .await?;
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
