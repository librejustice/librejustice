//! `cedh_application`, the cases the CEDH communicated to France, each settled
//! to the French ruling it attacks like a `REQUETE_CEDH_CONTRE` link.

use std::collections::HashSet;

use chrono::NaiveDate;

use super::{CedhVisit, DecisionRepository};
use crate::error::Result;

pub struct CedhApplicationRow {
    pub itemid: String,
    pub appnos: Vec<String>,
    pub name: String,
    pub introduced_on: Option<NaiveDate>,
    pub communicated_on: NaiveDate,
    pub articles: Vec<String>,
    pub text: String,
    pub target: Option<(String, NaiveDate)>,
}

/// A communicated case attacking a decision of a chronology, with the CEDH
/// ruling the corpus holds under one of its application numbers.
pub struct CommunicatedStep {
    pub itemid: String,
    pub appnos: Vec<String>,
    pub communicated_on: NaiveDate,
    pub target_public_id: String,
    pub ruling: Option<HeldCedhRuling>,
}

pub struct HeldCedhRuling {
    pub public_id: String,
    pub jurisdiction_code: Option<String>,
    pub solution_uid: Option<String>,
}

pub struct PendingCedhApplication {
    pub itemid: String,
    pub communicated_on: NaiveDate,
    pub text: String,
    pub court: String,
    pub date: NaiveDate,
}

impl DecisionRepository<'_> {
    pub async fn cedh_application_itemids(&self) -> Result<HashSet<String>> {
        let rows = self
            .conn
            .query("SELECT itemid FROM cedh_application", &[])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    pub async fn insert_cedh_application(&self, row: &CedhApplicationRow) -> Result<bool> {
        let (court, date) = row.target.clone().unzip();
        let inserted = self
            .conn
            .execute(
                "INSERT INTO cedh_application
                   (itemid, appnos, name, introduced_on, communicated_on, articles, text,
                    target_court, target_date)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (itemid) DO NOTHING",
                &[
                    &row.itemid,
                    &row.appnos,
                    &row.name,
                    &row.introduced_on,
                    &row.communicated_on,
                    &row.articles,
                    &row.text,
                    &court,
                    &date,
                ],
            )
            .await?;
        Ok(inserted == 1)
    }

    pub async fn pending_cedh_applications(
        &self,
        visit: CedhVisit,
    ) -> Result<Vec<PendingCedhApplication>> {
        let rows = self
            .conn
            .query(
                "SELECT a.itemid, a.communicated_on, a.text, a.target_court, a.target_date
                 FROM cedh_application a
                 WHERE a.target_decision_id IS NULL
                   AND a.target_court IS NOT NULL AND a.target_date IS NOT NULL
                   AND (a.visited_at IS NULL
                        OR ($1 AND EXISTS (
                            SELECT 1 FROM decisions c
                            WHERE c.jurisdiction_code = a.target_court
                              AND c.date_lecture = a.target_date
                              AND c.updated_at > a.visited_at)))",
                &[&(visit == CedhVisit::Changed)],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| PendingCedhApplication {
                itemid: r.get(0),
                communicated_on: r.get(1),
                text: r.get(2),
                court: r.get(3),
                date: r.get(4),
            })
            .collect())
    }

    pub async fn communicated_steps(&self, chain: &[i64]) -> Result<Vec<CommunicatedStep>> {
        let rows = self
            .conn
            .query(
                "SELECT a.itemid, a.appnos, a.communicated_on, t.public_id
                 FROM cedh_application a
                 JOIN decisions t ON t.id = a.target_decision_id
                 WHERE a.target_decision_id = ANY($1)",
                &[&chain],
            )
            .await?;
        let mut steps = Vec::with_capacity(rows.len());
        for r in rows {
            let appnos: Vec<String> = r.get(1);
            let ruling = self
                .conn
                .query_opt(
                    "SELECT public_id, jurisdiction_code, solution_uid
                     FROM decisions
                     WHERE deleted_at IS NULL AND jurisdiction_type = 'CEDH'
                       AND lj_docket_keys(docket_numbers) && lj_docket_keys($1::text[])
                     ORDER BY date_lecture DESC NULLS LAST
                     LIMIT 1",
                    &[&appnos],
                )
                .await?
                .map(|d| HeldCedhRuling {
                    public_id: d.get(0),
                    jurisdiction_code: d.get(1),
                    solution_uid: d.get(2),
                });
            steps.push(CommunicatedStep {
                itemid: r.get(0),
                appnos,
                communicated_on: r.get(2),
                target_public_id: r.get(3),
                ruling,
            });
        }
        Ok(steps)
    }

    /// A pending CEDH link and a communicated case of the same applications,
    /// naming the same court and day, take the decision the other settled to.
    pub async fn settle_cedh_steps_by_application(&self) -> Result<(u64, u64)> {
        let links = self
            .conn
            .execute(
                "WITH m AS (
                     SELECT l.id, min(a.target_decision_id) AS target,
                            count(DISTINCT a.target_decision_id) AS n
                     FROM decision_links l
                     JOIN decisions d ON d.id = l.decision_id
                     JOIN cedh_application a
                       ON a.target_decision_id IS NOT NULL
                      AND a.target_court = l.target_court AND a.target_date = l.target_date
                      AND a.appnos @> d.docket_numbers AND a.appnos <@ d.docket_numbers
                     WHERE l.link_type = 'REQUETE_CEDH_CONTRE' AND l.target_decision_id IS NULL
                     GROUP BY l.id)
                 UPDATE decision_links l SET target_decision_id = m.target, visited_at = now()
                 FROM m WHERE m.id = l.id AND m.n = 1",
                &[],
            )
            .await?;
        let applications = self
            .conn
            .execute(
                "WITH m AS (
                     SELECT a.itemid, min(l.target_decision_id) AS target,
                            count(DISTINCT l.target_decision_id) AS n
                     FROM cedh_application a
                     JOIN decision_links l
                       ON l.link_type = 'REQUETE_CEDH_CONTRE' AND l.target_decision_id IS NOT NULL
                      AND l.target_court = a.target_court AND l.target_date = a.target_date
                     JOIN decisions d
                       ON d.id = l.decision_id AND d.deleted_at IS NULL
                      AND d.docket_numbers @> a.appnos AND d.docket_numbers <@ a.appnos
                     WHERE a.target_decision_id IS NULL
                     GROUP BY a.itemid)
                 UPDATE cedh_application a SET target_decision_id = m.target, visited_at = now()
                 FROM m WHERE m.itemid = a.itemid AND m.n = 1",
                &[],
            )
            .await?;
        Ok((links, applications))
    }

    pub async fn visit_cedh_application(&self, itemid: &str, target: Option<i64>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE cedh_application SET target_decision_id = $2, visited_at = now()
                 WHERE itemid = $1",
                &[&itemid, &target],
            )
            .await?;
        Ok(())
    }
}
