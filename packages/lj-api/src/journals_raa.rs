//! The `raa` fond, the prefectures' recueils searched by their words, newest first: the
//! département is their one axis, and a recueil taken down is neither listed nor counted.

use lj_dtos::{
    journal_acteur_role, journal_fond_label, FacetChoice, FacetTag, JournalFacetAxis, JournalHit,
    JournalSearchRequest,
};
use lj_norm::referential_labels::departement_label;
use lj_store::repository::{DecisionRepository, RaaHitRow};
use lj_vocab::JOURNAL_FOND_RAA;

use crate::error::Result;
use crate::journals_gazette::by_count;
use crate::state::AppState;

fn question(req: &JournalSearchRequest) -> Option<&str> {
    req.q.as_deref().map(str::trim).filter(|q| !q.is_empty())
}

/// A page of the fond, newest first, and its total. Under another fond's filter it answers nothing.
pub async fn page(
    state: &AppState,
    req: &JournalSearchRequest,
    avec_hits: bool,
) -> Result<(Vec<JournalHit>, i64)> {
    if !req.famille.is_empty()
        || !req.cpv.is_empty()
        || !req.metier.is_empty()
        || req.echeance_jours.is_some()
    {
        return Ok((Vec::new(), 0));
    }
    let text = question(req);
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let total = repo.raa_total(text, &req.zone).await?;
    if !avec_hits {
        return Ok((Vec::new(), total));
    }
    let rows = repo
        .raa_search(
            text,
            &req.zone,
            i64::from(req.limit.min(100)),
            i64::from(req.offset),
        )
        .await?;
    Ok((rows.iter().map(hit_from).collect(), total))
}

/// The département axis, counted without its own filter.
pub async fn axes(state: &AppState, req: &JournalSearchRequest) -> Result<Vec<JournalFacetAxis>> {
    let conn = state.pool.get().await?;
    let counts = DecisionRepository::new(&conn)
        .raa_depts(question(req))
        .await?;
    Ok(vec![JournalFacetAxis {
        key: "zone".to_string(),
        label: "Département".to_string(),
        choices: by_count(
            counts
                .into_iter()
                .map(|(code, count)| FacetChoice {
                    label: departement_label(&code).unwrap_or(&code).to_string(),
                    value: code,
                    count,
                    parent: None,
                    hint: None,
                })
                .collect(),
        ),
    }])
}

/// Tantivy escapes the snippet it cuts as HTML.
fn unescape(snippet: &str) -> String {
    snippet
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

fn hit_from(r: &RaaHitRow) -> JournalHit {
    JournalHit {
        uid: format!("raa:{}", r.id),
        fond: JOURNAL_FOND_RAA.to_string(),
        fond_label: journal_fond_label(JOURNAL_FOND_RAA)
            .unwrap_or_default()
            .to_string(),
        titre: r.title.clone(),
        description: r
            .excerpt
            .as_deref()
            .map(unescape)
            .filter(|e| !e.trim().is_empty()),
        acteur: crate::notices::prefecture(&r.publisher).unwrap_or_default(),
        acteur_role: journal_acteur_role(JOURNAL_FOND_RAA).to_string(),
        jurisdiction: "FR".to_string(),
        journal: None,
        zones: r
            .dept
            .iter()
            .map(|code| FacetTag {
                key: code.clone(),
                label: departement_label(code).unwrap_or(code).to_string(),
            })
            .collect(),
        famille: None,
        cpv: None,
        cpv_deduit: false,
        metiers: Vec::new(),
        metiers_deduits: false,
        date_publication: r.published.map(|d| d.to_string()).unwrap_or_default(),
        echeance: None,
        closed: false,
        attribue: false,
        url_action: r.url.clone(),
        reference: None,
        procedure: None,
        first_seen_at: None,
        canaux: vec!["raa".to_string()],
    }
}
