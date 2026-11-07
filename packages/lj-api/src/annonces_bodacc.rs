//! The `bodacc` fond, searched in the base: a notice states a fact already
//! past, so it has neither deadline nor CPV, and its axes are the family and the
//! department.

use lj_dtos::{
    annonce_acteur_role, annonce_fond_label, AnnonceFacetAxis, AnnonceHit, AnnonceSearchRequest,
    FacetChoice, FacetTag,
};
use lj_gazette::bodacc::{Avis, Famille};
use lj_gazette::Source;
use lj_store::repository::{
    BodaccNoticeRow, DecisionRepository, GazetteAxis, GazetteFilter, GazetteQuery,
};
use lj_vocab::ANNONCE_FOND_BODACC;

use crate::annonces_gazette::{self, Depts};
use crate::error::Result;
use crate::state::AppState;

/// Page consultable de l'annonce sur le site officiel — le « où agir » du fond.
fn url_annonce(r: &Record) -> String {
    format!(
        "https://www.bodacc.fr/pages/annonce/detail-annonce/{}/{}/{}",
        r.publicationavis, r.parution, r.numeroannonce
    )
}

#[derive(Debug)]
struct Record {
    id: String,
    publicationavis: String,
    parution: String,
    numeroannonce: i64,
    dateparution: Option<String>,
    familleavis: Option<String>,
    familleavis_lib: Option<String>,
    numerodepartement: Option<String>,
    departement_nom_officiel: Option<String>,
    typeavis_lib: Option<String>,
    tribunal: Option<String>,
    commercant: Option<String>,
    ville: Option<String>,
}

fn filter(req: &AnnonceSearchRequest, depts: Option<Vec<i16>>) -> GazetteFilter {
    GazetteFilter {
        source: Source::Bodacc,
        query: annonces_gazette::query(req, Source::Bodacc),
        classes: (!req.famille.is_empty()).then(|| {
            Famille::ALL
                .into_iter()
                .filter(|f| req.famille.iter().any(|t| t == f.ods_code()))
                .map(|f| f as i16)
                .collect()
        }),
        depts,
    }
}

/// A page of the fond, newest first, and its total.
pub async fn page(
    state: &AppState,
    req: &AnnonceSearchRequest,
    avec_hits: bool,
) -> Result<(Vec<AnnonceHit>, i64)> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let depts = match req.zone.is_empty() {
        true => None,
        false => Depts::of(&repo, Source::Bodacc).await?.codes(&req.zone),
    };
    let f = filter(req, depts);
    let total = repo.gazette_total(&f).await?;
    if !avec_hits {
        return Ok((Vec::new(), total));
    }
    let rows = repo
        .bodacc_search(&f, i64::from(req.limit.min(100)), i64::from(req.offset))
        .await?;
    Ok((rows.iter().map(hit_from_row).collect(), total))
}

/// The BODACC notices of a SIREN for the entity page.
pub(crate) async fn pour_entite(
    state: &AppState,
    siren: &str,
    limit: usize,
) -> std::result::Result<(Vec<AnnonceHit>, i64), ()> {
    let siren: i32 = siren.parse().map_err(|_| ())?;
    let conn = state.pool.get().await.map_err(|e| {
        tracing::warn!(error = %e, "bodacc entity notices: no connection");
    })?;
    let repo = DecisionRepository::new(&conn);
    let f = GazetteFilter {
        source: Source::Bodacc,
        query: GazetteQuery::Siren(siren),
        classes: None,
        depts: None,
    };
    let fetched = async {
        let rows = repo.bodacc_search(&f, limit as i64, 0).await?;
        Ok::<_, lj_store::error::StoreError>((rows, repo.gazette_total(&f).await?))
    };
    let (rows, total) = fetched.await.map_err(|e| {
        tracing::warn!(error = %e, "bodacc entity notices");
    })?;
    Ok((rows.iter().map(hit_from_row).collect(), total))
}

fn hit_from_row(r: &BodaccNoticeRow) -> AnnonceHit {
    let (letter, parution, num) = lj_gazette::bodacc::split_id(r.id);
    let famille = Famille::from_code(r.famille);
    let typeavis_lib = match Avis::from_code(r.avis) {
        Avis::Initial => Some("Avis initial"),
        Avis::Rectificatif => Some("Avis rectificatif"),
        Avis::Annulation => Some("Avis d'annulation"),
        Avis::Inconnu => None,
    };
    let commercant = match (&r.name, &r.first_name) {
        (Some(n), Some(p)) => Some(format!("{n} {p}")),
        (n, _) => n.clone(),
    };
    let record = Record {
        id: format!("{letter}{parution}{num}"),
        publicationavis: letter.to_string(),
        parution: parution.to_string(),
        numeroannonce: i64::from(num),
        dateparution: Some(r.date.to_string()),
        familleavis: Some(famille.ods_code().to_string()),
        familleavis_lib: Some(famille_label(famille).to_string()),
        numerodepartement: r.dept.clone(),
        departement_nom_officiel: r
            .dept
            .as_deref()
            .and_then(lj_norm::referential_labels::departement_label)
            .map(str::to_string),
        typeavis_lib: typeavis_lib.map(str::to_string),
        tribunal: r.tribunal.clone(),
        commercant,
        ville: r.ville.clone(),
    };
    hit_from(&record)
}

/// The family and department axes, each counted without its own filter.
pub async fn axes(state: &AppState, req: &AnnonceSearchRequest) -> Result<Vec<AnnonceFacetAxis>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let depts = Depts::of(&repo, Source::Bodacc).await?;
    let f = filter(req, depts.codes(&req.zone));
    let familles = repo.gazette_facet(&f, GazetteAxis::Class).await?;
    let zones = repo.gazette_facet(&f, GazetteAxis::Dept).await?;
    Ok(vec![
        AnnonceFacetAxis {
            key: "famille".to_string(),
            label: "Famille d'avis".to_string(),
            choices: annonces_gazette::by_count(
                familles
                    .into_iter()
                    .map(|(code, count)| {
                        let famille = Famille::from_code(code);
                        FacetChoice {
                            value: famille.ods_code().to_string(),
                            label: famille_label(famille).to_string(),
                            count,
                            parent: None,
                            hint: None,
                        }
                    })
                    .collect(),
            ),
        },
        AnnonceFacetAxis {
            key: "zone".to_string(),
            label: "Département".to_string(),
            choices: depts.choices(zones),
        },
    ])
}

/// Familles d'avis du BODACC, libellées comme le site officiel les nomme.
fn famille_label(famille: Famille) -> &'static str {
    match famille {
        Famille::Creation => "Créations",
        Famille::Immatriculation => "Immatriculations",
        Famille::Modification => "Modifications",
        Famille::Radiation => "Radiations",
        Famille::Vente => "Ventes et cessions",
        Famille::Dpc => "Dépôts des comptes",
        Famille::Collective => "Procédures collectives",
        Famille::Conciliation => "Procédures de conciliation",
        Famille::RetablissementProfessionnel => "Rétablissements professionnels",
        Famille::Divers => "Avis divers",
        Famille::Inconnue => "Autres annonces",
    }
}

fn hit_from(r: &Record) -> AnnonceHit {
    // L'acteur d'une annonce BODACC est le commerçant ; à défaut, la juridiction
    // qui la publie situe encore la ligne.
    let acteur = r
        .commercant
        .clone()
        .or_else(|| r.tribunal.clone())
        .unwrap_or_default();
    let titre = match (&r.typeavis_lib, &r.familleavis_lib, &r.ville) {
        (Some(t), Some(f), Some(v)) => format!("{f} — {t} ({v})"),
        (_, Some(f), Some(v)) => format!("{f} ({v})"),
        (_, Some(f), None) => f.clone(),
        _ => "Annonce BODACC".to_string(),
    };
    AnnonceHit {
        uid: format!("bodacc:{}", r.id),
        fond: ANNONCE_FOND_BODACC.to_string(),
        fond_label: annonce_fond_label(ANNONCE_FOND_BODACC)
            .unwrap_or_default()
            .to_string(),
        titre,
        description: None,
        acteur,
        acteur_role: annonce_acteur_role(ANNONCE_FOND_BODACC).to_string(),
        zones: r
            .numerodepartement
            .iter()
            .map(|c| FacetTag {
                label: r
                    .departement_nom_officiel
                    .clone()
                    .unwrap_or_else(|| c.clone()),
                key: c.clone(),
            })
            .collect(),
        famille: r.familleavis.as_ref().map(|f| FacetTag {
            key: f.clone(),
            label: r.familleavis_lib.clone().unwrap_or_else(|| f.clone()),
        }),
        cpv: None,
        cpv_deduit: false,
        metiers: Vec::new(),
        metiers_deduits: false,
        date_publication: r.dateparution.clone().unwrap_or_default(),
        // Une annonce constate un fait déjà survenu : aucune échéance, donc
        // aucun `VEVENT` dans un agenda et aucune fenêtre « ferme sous N jours ».
        echeance: None,
        closed: false,
        attribue: false,
        url_action: url_annonce(r),
        reference: Some(r.id.clone()),
        procedure: r.typeavis_lib.clone(),
        first_seen_at: None,
        canaux: vec!["bodacc".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projette_un_enregistrement_dans_le_hit_commun() {
        let r = Record {
            id: "A20260100042".into(),
            publicationavis: "A".into(),
            parution: "20260100".into(),
            numeroannonce: 42,
            dateparution: Some("2026-08-05".into()),
            familleavis: Some("creation".into()),
            familleavis_lib: Some("Créations".into()),
            numerodepartement: Some("31".into()),
            departement_nom_officiel: Some("Haute-Garonne".into()),
            typeavis_lib: Some("Avis initial".into()),
            tribunal: Some("Greffe du Tribunal de Commerce de Toulouse".into()),
            commercant: Some("DURAND, Alice".into()),
            ville: Some("Toulouse".into()),
        };
        let hit = hit_from(&r);
        assert_eq!(hit.uid, "bodacc:A20260100042");
        assert_eq!(hit.acteur, "DURAND, Alice");
        assert_eq!(hit.acteur_role, "Entreprise");
        assert_eq!(hit.titre, "Créations — Avis initial (Toulouse)");
        assert_eq!(
            hit.zones.iter().map(|z| z.key.as_str()).collect::<Vec<_>>(),
            ["31"]
        );
        assert!(hit.echeance.is_none() && hit.cpv.is_none());
        assert_eq!(
            hit.url_action,
            "https://www.bodacc.fr/pages/annonce/detail-annonce/A/20260100/42"
        );
    }
}
