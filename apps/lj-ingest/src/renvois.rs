//! Job `renvois` : projette les renvois des analyses du Conseil d'État en
//! relation de citation (`decision_renvoi`), résout les cibles, puis pose sur
//! chaque cible le nom que ses citantes lui donnent (`decisions.case_name`) et
//! le réinjecte dans le titre, où la jambe BM25 titre le sert.

use std::collections::{BTreeMap, HashSet};

use anyhow::{anyhow, Result};
use lj_core::address::CaseAddress;
use lj_core::case_names::case_name_key;
use lj_extract::eponyme::parse_renvoi;
use lj_store::repository::{DecisionRepository, ElectedCaseName, Renvoi, TitleInputs};

use crate::case_names::{add_ordinal_keys, add_short_keys, NOM_TROP_COURANT};
use crate::config::Settings;

const LOT: i64 = 2_000;

pub async fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut apres, mut sources, mut lus, mut ecrits) = (0i64, 0usize, 0usize, 0u64);
    loop {
        let lot = repo.ariane_renvois_batch(apres, LOT).await?;
        if lot.is_empty() {
            break;
        }
        let mut ids: Vec<i64> = Vec::with_capacity(lot.len());
        let mut rows: Vec<Renvoi> = Vec::new();
        for (id, renvois) in &lot {
            apres = *id;
            sources += 1;
            ids.push(*id);
            let mut renvoi_no: i16 = 0;
            for brut in renvois {
                lus += 1;
                for c in parse_renvoi(brut) {
                    let numeros = if c.dockets.is_empty() {
                        vec![String::new()]
                    } else {
                        c.dockets.clone()
                    };
                    for num in numeros {
                        rows.push(Renvoi {
                            decision_id: *id,
                            renvoi_no,
                            address: CaseAddress::new(&c.juridiction, &num, &c.date_iso),
                            nom: c.nom.clone(),
                        });
                    }
                    renvoi_no += 1;
                }
            }
        }
        ecrits += repo.replace_renvois(&ids, &rows).await?;
    }
    let resolus = repo.resolve_pending_renvois().await?;
    let (elected, ferraille, courts, ordinals) = case_names(&repo).await?;
    let poses = repo.put_case_names(&elected).await?;
    let kept: Vec<i64> = elected.iter().map(|e| e.decision_id).collect();
    let effaces = repo.clear_case_names_except(&["renvoi"], &kept).await?;
    let titles = recomposed_titles(&repo.case_name_title_inputs().await?);
    let retitled = repo.set_search_titles(&titles).await?;
    println!(
        "renvois : {sources} décisions commentées, {lus} renvois lus, {ecrits} cibles écrites, \
         {resolus} résolues au fonds, {} noms d'usage dont {poses} changés et {} effacés, \
         {ferraille} mots trop courants écartés, {courts} clés courtes trop courantes écartées, \
         {ordinals} noms ordinaux appris des citations, {retitled} titres réécrits",
        elected.len(),
        effaces.len()
    );
    Ok(())
}

/// The names the renvois elect, those the corpus writes too often dropped, with
/// their short and ordinal keys.
async fn case_names(
    repo: &DecisionRepository<'_>,
) -> Result<(Vec<ElectedCaseName>, usize, usize, usize)> {
    let mut by_decision: BTreeMap<i64, ElectedCaseName> = BTreeMap::new();
    for (decision_id, name) in repo.elected_case_names().await? {
        let key = case_name_key(&name);
        if key.is_empty() {
            continue;
        }
        by_decision.insert(
            decision_id,
            ElectedCaseName {
                decision_id,
                name: Some(name),
                keys: vec![key],
                source: "renvoi",
            },
        );
    }

    let uniques: Vec<&str> = by_decision
        .values()
        .filter_map(|e| e.keys.first())
        .map(String::as_str)
        .collect();
    let courants: HashSet<String> = repo
        .frequent_names(&uniques, NOM_TROP_COURANT)
        .await?
        .into_iter()
        .collect();
    let avant = by_decision.len();
    by_decision.retain(|_, e| !e.keys.iter().any(|k| courants.contains(k)));
    let ferraille = avant - by_decision.len();
    let courts = add_short_keys(repo, &mut by_decision).await?;

    let ordinals = add_ordinal_keys(repo, &mut by_decision).await?;
    Ok((
        by_decision.into_values().collect(),
        ferraille,
        courts,
        ordinals,
    ))
}

/// Recompose le titre des décisions nommées, par la fonction qui l'a composé à
/// l'extraction. Rejouer ne change rien, et une ré-extraction, qui recompose
/// sans connaître le nom, est rattrapée à la passe suivante.
fn recomposed_titles(inputs: &[TitleInputs]) -> Vec<(i64, String)> {
    use lj_core::titles as t;
    let mut titles = Vec::new();
    for i in inputs {
        let Some(jurisdiction) = i
            .jurisdiction_label
            .clone()
            .or_else(|| t::jurisdiction_type_label(&i.jurisdiction_type).map(str::to_string))
        else {
            continue;
        };
        let named = t::decision_title(
            &jurisdiction,
            None,
            i.date_lecture.as_deref(),
            i.case_name.as_deref(),
            t::docket_display(&i.docket_numbers).as_deref(),
        );
        if i.search_title.as_deref() != Some(named.as_str()) {
            titles.push((i.decision_id, named));
        }
    }
    titles
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use lj_core::chamber::{ChamberId, Container, Seat};
    use lj_store::repository::{ExtractedFields, JurisdictionRow};

    /// The two composers of `search_title` must agree to the character. They do
    /// not share a code path: the extraction reads `ExtractedFields`, this pass
    /// reads the columns. A seat left on one side alone makes the difference
    /// test fire on every named decision, and every named title is rewritten for
    /// nothing.
    #[test]
    fn the_two_title_composers_agree() {
        let court = "Cour d'appel de Paris";
        let dockets = vec!["21/01234".to_string()];
        let civile = Seat {
            jurisdiction_type: "CA".to_string(),
            jurisdiction_code: Some("ca_paris".to_string()),
            container: Some(Container::Pole(5)),
            chamber: Some(ChamberId::Number(6)),
            specialty: vec!["matiere:CIVILE"],
            ..Default::default()
        };
        for seat in [None, Some(civile)] {
            for case_name in [None, Some("Danthony")] {
                let extracted = ExtractedFields {
                    jurisdiction: Some(JurisdictionRow {
                        source_code: "ca_paris".to_string(),
                        code: "ca_paris".to_string(),
                        jurisdiction_type: "CA".to_string(),
                        city: Some("Paris".to_string()),
                        label: court.to_string(),
                        place: None,
                    }),
                    date_lecture: NaiveDate::from_ymd_opt(2026, 1, 15),
                    docket_numbers: dockets.clone(),
                    seat: seat.clone(),
                    formation_uid: Some("formation:A_TROIS".to_string()),
                    office_uid: Some("office:JEX".to_string()),
                    ..Default::default()
                };
                let inputs = TitleInputs {
                    decision_id: 1,
                    jurisdiction_label: Some(court.to_string()),
                    jurisdiction_type: "CA".to_string(),
                    date_lecture: Some("2026-01-15".to_string()),
                    docket_numbers: dockets.clone(),
                    case_name: case_name.map(str::to_string),
                    // Never equal to the composition, so the pass always returns it.
                    search_title: None,
                };
                let a = lj_extract_rows::compose_search_title(
                    &extracted,
                    Some("CA"),
                    &std::collections::HashMap::new(),
                    case_name,
                )
                .expect("a court names a title");
                let b = recomposed_titles(std::slice::from_ref(&inputs));
                assert_eq!(b.len(), 1, "the pass returns the recomposed title");
                assert_eq!(a, b[0].1, "seat={seat:?} case_name={case_name:?}");
                assert!(!a.contains("chambre"), "the seat left the title: {a}");
            }
        }
    }
}
