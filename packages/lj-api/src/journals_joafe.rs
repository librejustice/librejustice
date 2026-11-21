//! The `joafe` fond, the Journal officiel des associations searched in the base:
//! a notice states a fact already past, so it has neither deadline nor CPV, and
//! its axes are the action and the department. The official PDF is the line's
//! link, never proxied.

use lj_dtos::{
    journal_acteur_role, journal_fond_label, FacetChoice, FacetTag, JournalFacetAxis, JournalHit,
    JournalSearchRequest,
};
use lj_gazette::joafe::Action;
use lj_gazette::Source;
use lj_store::repository::{
    DecisionRepository, GazetteFacets, GazetteFilter, GazetteQuery, JoafeNoticeRow,
};
use lj_vocab::JOURNAL_FOND_JOAFE;

use crate::error::Result;
use crate::journals_gazette::{self, Depts};
use crate::state::AppState;

#[derive(Debug)]
struct Record {
    id: Option<String>,
    dateparution: Option<String>,
    action: Option<Action>,
    typeavis: Option<String>,
    titre: Option<String>,
    objet: Option<String>,
    departement_code: Option<String>,
}

fn filter(req: &JournalSearchRequest, depts: Option<Vec<i16>>) -> GazetteFilter {
    GazetteFilter {
        source: Source::Joafe,
        query: journals_gazette::query(req, Source::Joafe),
        classes: (!req.famille.is_empty()).then(|| {
            Action::ALL
                .into_iter()
                .filter(|a| req.famille.iter().any(|t| t == a.slug()))
                .map(|a| a as i16)
                .collect()
        }),
        depts,
    }
}

/// A page of the fond, newest first, and its total.
pub async fn page(
    state: &AppState,
    req: &JournalSearchRequest,
    avec_hits: bool,
) -> Result<(Vec<JournalHit>, i64)> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let depts = match req.zone.is_empty() {
        true => None,
        false => Depts::of(&repo, Source::Joafe).await?.codes(&req.zone),
    };
    let f = filter(req, depts);
    let total = repo.gazette_total(&f).await?;
    if !avec_hits {
        return Ok((Vec::new(), total));
    }
    let rows = repo
        .joafe_search(&f, i64::from(req.limit.min(100)), i64::from(req.offset))
        .await?;
    Ok((rows.iter().map(hit_from_row).collect(), total))
}

/// The JOAFE notices of an RNA for the entity page.
pub(crate) async fn pour_entite(
    state: &AppState,
    rna: &str,
    limit: usize,
) -> std::result::Result<(Vec<JournalHit>, i64), ()> {
    let conn = state.pool.get().await.map_err(|e| {
        tracing::warn!(error = %e, "joafe entity notices: no connection");
    })?;
    let repo = DecisionRepository::new(&conn);
    let f = GazetteFilter {
        source: Source::Joafe,
        query: GazetteQuery::Rna(rna.to_string()),
        classes: None,
        depts: None,
    };
    let fetched = async {
        let rows = repo.joafe_search(&f, limit as i64, 0).await?;
        Ok::<_, lj_store::error::StoreError>((rows, repo.gazette_total(&f).await?))
    };
    let (rows, total) = fetched.await.map_err(|e| {
        tracing::warn!(error = %e, "joafe entity notices");
    })?;
    Ok((rows.iter().map(hit_from_row).collect(), total))
}

fn hit_from_row(r: &JoafeNoticeRow) -> JournalHit {
    let action = Action::from_code(r.action);
    let typeavis = match action {
        Action::Autre => r.type_label.clone(),
        known => Some(action_label(known).to_string()),
    };
    hit_from(&Record {
        id: Some(format!("{:012}", r.id)),
        dateparution: Some(r.date.to_string()),
        action: Some(action),
        typeavis,
        titre: r.titre.clone(),
        objet: r.objet.clone(),
        departement_code: r
            .dept
            .as_deref()
            .map(|d| lj_gazette::canonical_dept(d).into_owned()),
    })
}

/// The action and department axes, each counted without its own filter.
pub async fn axes(state: &AppState, req: &JournalSearchRequest) -> Result<Vec<JournalFacetAxis>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let depts = Depts::of(&repo, Source::Joafe).await?;
    let f = filter(req, depts.codes(&req.zone));
    let GazetteFacets {
        classes: actions,
        depts: zones,
    } = repo.gazette_facets(&f).await?;
    Ok(vec![
        JournalFacetAxis {
            key: "famille".to_string(),
            label: "Type d'avis".to_string(),
            choices: journals_gazette::by_count(
                actions
                    .into_iter()
                    .map(|(code, count)| {
                        let action = Action::from_code(code);
                        FacetChoice {
                            value: action.slug().to_string(),
                            label: action_label(action).to_string(),
                            count,
                            parent: None,
                            hint: None,
                        }
                    })
                    .collect(),
            ),
        },
        JournalFacetAxis {
            key: "zone".to_string(),
            label: "Département".to_string(),
            choices: depts.choices(zones),
        },
    ])
}

pub(crate) fn action_label(action: Action) -> &'static str {
    match action {
        Action::Creation => "Création",
        Action::Modification => "Modification",
        Action::Dissolution => "Dissolution",
        Action::Suspension => "Suspension d'activité",
        Action::Saisine => "Saisine",
        Action::Autre => "Autres avis",
    }
}

/// Official page, rebuilt from the parution id (`202600010183` → `20260001`,
/// `0183`).
pub(crate) fn detail_url(id: &str) -> Option<String> {
    (id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit())).then(|| {
        format!(
            "https://www.journal-officiel.gouv.fr/associations/detail-annonce/associations_b/{}/{}",
            &id[..8],
            &id[8..]
        )
    })
}

fn hit_from(r: &Record) -> JournalHit {
    let type_avis = r.typeavis.as_deref().unwrap_or("Annonce");
    let titre = match &r.titre {
        Some(nom) => format!("{type_avis} — {nom}"),
        None => type_avis.to_string(),
    };
    JournalHit {
        uid: format!("joafe:{}", r.id.as_deref().unwrap_or_default()),
        fond: JOURNAL_FOND_JOAFE.to_string(),
        fond_label: journal_fond_label(JOURNAL_FOND_JOAFE)
            .unwrap_or_default()
            .to_string(),
        titre,
        description: r.objet.clone(),
        acteur: r.titre.clone().unwrap_or_default(),
        acteur_role: journal_acteur_role(JOURNAL_FOND_JOAFE).to_string(),
        jurisdiction: "FR".to_string(),
        journal: None,
        zones: r
            .departement_code
            .iter()
            .map(|c| FacetTag {
                label: lj_norm::referential_labels::departement_label(c)
                    .unwrap_or(c)
                    .to_string(),
                key: c.clone(),
            })
            .collect(),
        famille: r.action.map(|a| FacetTag {
            key: a.slug().to_string(),
            label: action_label(a).to_string(),
        }),
        cpv: None,
        cpv_deduit: false,
        metiers: Vec::new(),
        metiers_deduits: false,
        date_publication: r.dateparution.clone().unwrap_or_default(),
        echeance: None,
        closed: false,
        attribue: false,
        url_action: r.id.as_deref().and_then(detail_url).unwrap_or_default(),
        reference: r.id.clone(),
        procedure: r.typeavis.clone(),
        first_seen_at: None,
        canaux: vec![JOURNAL_FOND_JOAFE.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projette_une_parution_dans_le_hit_commun() {
        let r = Record {
            id: Some("associations_1234".into()),
            dateparution: Some("2026-03-14".into()),
            action: Some(Action::Creation),
            typeavis: Some("Création".into()),
            titre: Some("LES AMIS DU CANAL".into()),
            objet: Some("Promouvoir la navigation fluviale".into()),
            departement_code: Some("31".into()),
        };
        let hit = hit_from(&r);
        assert_eq!(hit.uid, "joafe:associations_1234");
        assert_eq!(hit.titre, "Création — LES AMIS DU CANAL");
        assert_eq!(hit.acteur_role, "Association");
        assert_eq!(
            hit.zones
                .iter()
                .map(|z| z.label.as_str())
                .collect::<Vec<_>>(),
            ["Haute-Garonne"]
        );
        assert_eq!(
            hit.famille.as_ref().map(|f| f.label.as_str()),
            Some("Création")
        );
        assert!(hit.echeance.is_none());
    }
}
