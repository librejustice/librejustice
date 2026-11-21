//! Settles the pending CEDH steps, `REQUETE_CEDH_CONTRE` links and communicated cases, among the decisions of the ruling's court and day.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_extract::cedh::{fold, Candidate, CedhFacts};
use lj_store::repository::{CedhVisit, DecisionRepository};

use crate::config::Settings;

pub async fn settle_cedh_steps(visit: CedhVisit) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let links = repo.pending_cedh_links(visit).await?;
    let mut settled = 0usize;
    for link in &links {
        let facts = repo
            .cedh_text(link.decision_id)
            .await?
            .and_then(|(date, text)| CedhFacts::read(&text, &date.to_string()));
        let target = settle(&repo, facts, &link.court, link.date).await?;
        settled += usize::from(target.is_some());
        repo.visit_cedh_link(link.id, target).await?;
    }
    tracing::info!(
        ?visit,
        read = links.len(),
        settled,
        "pending CEDH links read"
    );

    let applications = repo.pending_cedh_applications(visit).await?;
    let mut settled = 0usize;
    for application in &applications {
        let facts = CedhFacts::read(&application.text, &application.communicated_on.to_string());
        let target = settle(&repo, facts, &application.court, application.date).await?;
        settled += usize::from(target.is_some());
        repo.visit_cedh_application(&application.itemid, target)
            .await?;
    }
    tracing::info!(
        ?visit,
        read = applications.len(),
        settled,
        "pending CEDH communicated cases read"
    );

    let (links, applications) = repo.settle_cedh_steps_by_application().await?;
    tracing::info!(
        links,
        applications,
        "CEDH steps settled by their application number"
    );
    Ok(())
}

async fn settle(
    repo: &DecisionRepository<'_>,
    facts: Option<CedhFacts>,
    court: &str,
    date: NaiveDate,
) -> Result<Option<i64>> {
    let Some(facts) = facts else {
        return Ok(None);
    };
    let candidates: Vec<Candidate> = repo
        .cedh_candidates(court, date)
        .await?
        .into_iter()
        .map(|c| Candidate {
            id: c.id,
            solution: c
                .solution_uid
                .and_then(|s| s.split_once(':').map(|(_, leaf)| leaf.to_string())),
            dockets: c.dockets,
            attacked: c
                .attacked
                .into_iter()
                .map(|(date, court)| (date.to_string(), court))
                .collect(),
            text: fold(&c.text),
        })
        .collect();
    Ok(facts.settle(&candidates))
}
