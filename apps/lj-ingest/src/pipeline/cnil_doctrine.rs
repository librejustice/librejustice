//! The CNIL's référentiels, règlements types and recommandations, written to the norms as its doctrine.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_parse::CnilDoctrine;
use lj_store::repository::{BodyWrite, DecisionRepository, LegalTextRow};

const NATURE: &str = "CNIL";
const JO_NATURE: &str = "DELIBERATION";

#[derive(Debug, Default)]
pub(super) struct Written {
    pub texts: usize,
    pub jo_copies: usize,
}

/// Writes the deliberation to the norms, then folds into it the JO's copies of it.
pub(super) async fn write(
    repo: &DecisionRepository<'_>,
    d: &CnilDoctrine,
    written: &mut Written,
) -> Result<()> {
    let date = parse_date(&d.date_texte)?;
    let date_publi = d.date_publi.as_deref().map(parse_date).transpose()?;
    let instrument_key = lj_extract::instrument_key::instrument_key(
        NATURE,
        Some(&d.date_texte),
        d.number.as_deref(),
    );
    let status = match d.status.as_str() {
        "VIGUEUR" | "MODIFIE" => "VIGUEUR",
        _ => "ABROGE",
    };
    repo.upsert_legal_text(&LegalTextRow {
        text_uid: d.id.clone(),
        jurisdiction: "FR".to_string(),
        title: d.title.clone(),
        title_key: lj_extract::extract::normalize_instrument(&d.title),
        nature: NATURE.to_string(),
        last_modified: date_publi.or(Some(date)),
        date_texte: Some(date),
        date_publi,
        eli: None,
        nor: d.nor.clone(),
        instrument_key: instrument_key.clone(),
        status: Some(status.to_string()),
    })
    .await
    .map_err(|e| anyhow!("upsert_legal_text {}: {e}", d.id))?;
    repo.write_text_bodies(
        "cnil",
        &[(d.id.clone(), d.body.clone())],
        BodyWrite::Replace,
    )
    .await
    .map_err(|e| anyhow!("write_text_bodies {}: {e}", d.id))?;
    written.texts += 1;
    let Some(number) = instrument_key.as_deref().and_then(number) else {
        return Ok(());
    };
    for (uid, title, _) in repo.texts_of_nature_on(JO_NATURE, date).await? {
        if !uid.starts_with("JORFTEXT") || !is_jo_copy(&d.title, number, &title) {
            continue;
        }
        repo.adopt_slug(&uid, &d.id)
            .await
            .map_err(|e| anyhow!("adopt_slug {uid} into {}: {e}", d.id))?;
        if repo
            .collapse_legal_text(&uid, &d.id)
            .await
            .map_err(|e| anyhow!("collapse_legal_text {uid} into {}: {e}", d.id))?
            .is_some()
        {
            written.jo_copies += 1;
        }
    }
    Ok(())
}

/// The JO text is the copy of a CNIL deliberation the norms already hold.
pub(super) async fn is_held_copy(repo: &DecisionRepository<'_>, jo: &LegalTextRow) -> Result<bool> {
    let Some(date) = jo.date_texte.filter(|_| jo.nature == JO_NATURE) else {
        return Ok(false);
    };
    Ok(repo
        .texts_of_nature_on(NATURE, date)
        .await?
        .iter()
        .any(|(_, title, key)| {
            key.as_deref()
                .and_then(number)
                .is_some_and(|n| is_jo_copy(title, n, &jo.title))
        }))
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| anyhow!("CNIL date {s:?}: {e}"))
}

fn number(instrument_key: &str) -> Option<&str> {
    instrument_key.rsplit('|').next()
}

/// A JO deliberation of the same date is the CNIL's when its title carries the number and names
/// the CNIL or shares most of the CNIL title's words: the CRE numbers its deliberations alike.
fn is_jo_copy(cnil_title: &str, number: &str, jo_title: &str) -> bool {
    let jo = lj_fold::fold(jo_title);
    let digit = |i: usize| jo.as_bytes().get(i).is_some_and(u8::is_ascii_digit);
    let numbered = jo
        .match_indices(&lj_fold::fold(number))
        .any(|(i, m)| !i.checked_sub(1).is_some_and(digit) && !digit(i + m.len()));
    if !numbered {
        return false;
    }
    if jo.contains("cnil") || jo.contains("informatique") {
        return true;
    }
    let (a, b) = (words(cnil_title), words(jo_title));
    let shared = a.intersection(&b).count();
    shared * 5 >= a.len().min(b.len()).max(1) * 3
}

const MONTHS: &[&str] = &[
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

/// The words of four letters or more, months and « délibération » aside.
fn words(title: &str) -> HashSet<String> {
    lj_fold::fold(title)
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| w.len() >= 4 && *w != "deliberation" && !MONTHS.contains(w))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::is_jo_copy;

    #[test]
    fn a_jo_deliberation_is_the_cnils_on_its_number_and_its_subject() {
        let cnil = "Délibération n° 2019-001 du 10 janvier 2019 portant règlement type relatif à la mise en œuvre de dispositifs ayant pour finalité le contrôle d'accès par authentification biométrique";
        assert!(is_jo_copy(
            cnil,
            "2019-001",
            "Délibération n° 2019-001 du 10 janvier 2019 portant règlement type relatif à la mise en œuvre de dispositifs ayant pour finalité le contrôle d'accès"
        ));
        assert!(!is_jo_copy(
            cnil,
            "2019-001",
            "Délibération n° 2019-001 du 10 janvier 2019 portant décision sur un tarif non péréqué d'utilisation des réseaux publics de distribution de gaz naturel"
        ));
        assert!(!is_jo_copy(
            cnil,
            "2019-001",
            "Délibération n° 2019-0012 du 10 janvier 2019 portant règlement type relatif à la mise en œuvre de dispositifs"
        ));
        assert!(is_jo_copy(
            "Délibération portant adoption d'une recommandation concernant l'usage des autocommutateurs",
            "84-31",
            "Délibération n°84-31 du 18 septembre 1984 DE LA COMMISSION NATIONALE INFORMATIQUE ET LIBERTES"
        ));
    }
}
