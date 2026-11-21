//! The `jo` fond, the official journals' publications searched by their words, newest
//! first: no axis of its own, and an act CRPA R. 221-15 or R. 221-16 protects never listed nor counted.

use std::sync::Arc;
use std::time::Duration;

use lj_dtos::{journal_acteur_role, journal_fond_label, JournalHit, JournalSearchRequest};
use lj_norm::applicability::journal_place;
use lj_norm::individual::{is_protected_act, PROTECTED_WORDS};
use lj_store::repository::{DecisionRepository, JoActRow};
use lj_vocab::JOURNAL_FOND_JO;

use crate::error::{validation, ApiError, Result};
use crate::state::AppState;

/// How often the hidden acts reload, inside their cache lifetime so that no search waits on the
/// load, 1.5 s for some 25,000 titles.
const HIDDEN_REFRESH: Duration = Duration::from_secs(50 * 60);

/// The journals a search reads: those placed in the reader's scope, narrowed to the place or
/// the journal `jurisdiction` names; every journal when the request names neither.
pub async fn journals(state: &AppState, req: &JournalSearchRequest) -> Result<Option<Vec<String>>> {
    if req.jurisdiction.is_none() && req.home.is_none() {
        return Ok(None);
    }
    let conn = state.pool.get().await?;
    let codes = DecisionRepository::new(&conn).journal_codes().await?;
    let scope = req.home.as_deref().map(crate::jurisdictions::scope_seats);
    select(&codes, req.jurisdiction.as_deref(), scope.as_deref())
        .map(Some)
        .map_err(|paths| {
            let named = req.jurisdiction.as_deref().unwrap_or_default();
            let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
            ApiError::Unprocessable(validation::enum_error(
                &["query", "jurisdiction"],
                named,
                &paths,
            ))
        })
}

/// The codes placed in `scope` and under `jurisdiction`, a place or `place.code`; a
/// `jurisdiction` naming no journal gets every place and path it could name.
fn select(
    codes: &[String],
    jurisdiction: Option<&str>,
    scope: Option<&[String]>,
) -> std::result::Result<Vec<String>, Vec<String>> {
    let named = jurisdiction.map(str::to_ascii_uppercase);
    let (place, journal) = match named.as_deref().map(|n| n.split_once('.')) {
        Some(Some((place, journal))) => (Some(place), Some(journal)),
        Some(None) => (named.as_deref(), None),
        None => (None, None),
    };
    let read: Vec<String> = codes
        .iter()
        .filter(|code| {
            let at = journal_place(code);
            scope.is_none_or(|seats| seats.iter().any(|s| s == at))
                && place.is_none_or(|p| p == at)
                && journal.is_none_or(|j| j == code.as_str())
        })
        .cloned()
        .collect();
    if named.is_none() || !read.is_empty() {
        return Ok(read);
    }
    let mut paths: Vec<String> = codes
        .iter()
        .flat_map(|code| {
            let at = journal_place(code);
            [at.to_string(), format!("{at}.{code}")]
        })
        .collect();
    paths.sort();
    paths.dedup();
    Err(paths)
}

/// A page of the fond, newest first, and its total. Under another fond's filter, or in a scope
/// holding no journal, it answers nothing.
pub async fn page(
    state: &AppState,
    req: &JournalSearchRequest,
    journals: Option<&[String]>,
    avec_hits: bool,
) -> Result<(Vec<JournalHit>, i64)> {
    if !req.zone.is_empty()
        || !req.famille.is_empty()
        || !req.cpv.is_empty()
        || !req.metier.is_empty()
        || req.echeance_jours.is_some()
        || journals.is_some_and(<[String]>::is_empty)
    {
        return Ok((Vec::new(), 0));
    }
    let hidden = hidden(state).await?;
    let text = req.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let total = repo.jo_total(text, journals, &hidden).await?;
    if !avec_hits {
        return Ok((Vec::new(), total));
    }
    let rows = repo
        .jo_acts(
            text,
            journals,
            &hidden,
            i64::from(req.limit.min(100)),
            i64::from(req.offset),
        )
        .await?;
    Ok((rows.iter().map(hit_from).collect(), total))
}

/// Reloads the hidden acts for as long as the server runs, the first time at start.
pub async fn keep_hidden(state: AppState) {
    let mut every = tokio::time::interval(HIDDEN_REFRESH);
    loop {
        every.tick().await;
        match load_hidden(&state).await {
            Ok(acts) => state.jo_hidden_cache.insert(String::new(), acts).await,
            Err(e) => tracing::warn!(error = %e, "jo hidden acts reload failed"),
        }
    }
}

/// The protected acts DILA left unmarked, recognised by their title.
async fn hidden(state: &AppState) -> Result<Arc<Vec<String>>> {
    state
        .jo_hidden_cache
        .try_get_with(String::new(), load_hidden(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("jo hidden acts: {e}")))
}

/// The protected acts of every French journal, the CRPA being French law.
async fn load_hidden(state: &AppState) -> Result<Arc<Vec<String>>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let french: Vec<String> = repo
        .journal_codes()
        .await?
        .into_iter()
        .filter(|code| journal_place(code) == "FR")
        .collect();
    let titles = repo
        .jo_unmarked_titles_with_any(&PROTECTED_WORDS.join(" "), &french)
        .await?;
    Ok(Arc::new(
        titles
            .into_iter()
            .filter(|(_, title)| is_protected_act(title))
            .map(|(uid, _)| uid)
            .collect(),
    ))
}

fn hit_from(r: &JoActRow) -> JournalHit {
    let channel = r.journal_code.to_ascii_lowercase();
    JournalHit {
        uid: format!("{channel}:{}", r.text_uid),
        fond: JOURNAL_FOND_JO.to_string(),
        fond_label: journal_fond_label(JOURNAL_FOND_JO)
            .unwrap_or_default()
            .to_string(),
        titre: r.title.clone(),
        description: r.excerpt.clone().filter(|e| !e.trim().is_empty()),
        acteur: r.issuer.clone().unwrap_or_default(),
        acteur_role: journal_acteur_role(JOURNAL_FOND_JO).to_string(),
        jurisdiction: journal_place(&r.journal_code).to_string(),
        journal: Some(
            r.journal_label
                .clone()
                .unwrap_or_else(|| r.journal_code.clone()),
        ),
        zones: Vec::new(),
        famille: None,
        cpv: None,
        cpv_deduit: false,
        metiers: Vec::new(),
        metiers_deduits: false,
        date_publication: r.date_publi.map(|d| d.to_string()).unwrap_or_default(),
        echeance: None,
        closed: false,
        attribue: false,
        url_action: match r.journal_code.as_str() {
            "JORF" => format!("https://www.legifrance.gouv.fr/jorf/id/{}", r.text_uid),
            _ => r.url.clone().unwrap_or_default(),
        },
        reference: r.nor.clone(),
        procedure: None,
        first_seen_at: None,
        canaux: vec![channel],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes() -> Vec<String> {
        ["JORF", "BO-INTERIEUR", "JOUE", "MB-VLG", "RO", "JO-DJ"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn a_jurisdiction_reads_its_place_or_one_journal_and_a_scope_its_places() {
        let read = |j: Option<&str>, scope: Option<&[&str]>| {
            let scope: Option<Vec<String>> =
                scope.map(|s| s.iter().map(|x| x.to_string()).collect());
            select(&codes(), j, scope.as_deref())
        };
        assert_eq!(read(Some("FR"), None).unwrap(), ["JORF", "BO-INTERIEUR"]);
        assert_eq!(read(Some("be.mb-vlg"), None).unwrap(), ["MB-VLG"]);
        assert_eq!(read(Some("DJ"), None).unwrap(), ["JO-DJ"]);
        assert_eq!(
            read(None, Some(&["FR", "EU", "INTL.CEDH"][..])).unwrap(),
            ["JORF", "BO-INTERIEUR", "JOUE"]
        );
        assert_eq!(read(None, Some(&["SN"][..])).unwrap(), Vec::<String>::new());
        let refused = read(Some("FR.RO"), None).unwrap_err();
        assert!(refused.contains(&"CH.RO".to_string()) && refused.contains(&"BE".to_string()));
    }
}
