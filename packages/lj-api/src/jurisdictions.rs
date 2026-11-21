//! The reader's jurisdiction, and the seats a search held within it reads.

use lj_dtos::ReaderJurisdiction;

use crate::error::{validation, ApiError, Result};
use crate::referential::Referential;

/// The states whose courts hold decisions, France first, then by name. A root
/// that is an organisation's court, `EU` or `OHADA`, is no state.
pub fn known(refs: &Referential) -> Vec<ReaderJurisdiction> {
    let organisations: Vec<&str> = lj_norm::applicability::journals()
        .iter()
        .filter_map(|j| j.courts)
        .collect();
    let mut states: Vec<ReaderJurisdiction> = refs
        .seat_nodes()
        .iter()
        .filter(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_uppercase()))
        .filter(|p| !organisations.contains(&p.as_str()))
        .map(|p| {
            let code = p.to_ascii_lowercase();
            ReaderJurisdiction {
                seats: scope_seats(&code),
                label: refs.label(&format!("geo:{p}")).to_string(),
                code,
            }
        })
        .collect();
    states.sort_by_cached_key(|j| (j.code != "fr", lj_fold::fold(&j.label)));
    states
}

/// A jurisdiction code as the addresses write it, two lower-case letters.
pub(crate) fn parse_home(value: &str) -> Result<String> {
    if value.len() == 2 && value.bytes().all(|b| b.is_ascii_lowercase()) {
        Ok(value.to_string())
    } else {
        Err(ApiError::Unprocessable(validation::value_error(
            &["query", "home"],
            value,
            "a jurisdiction is two lower-case letters",
        )))
    }
}

/// The jurisdiction's own order, then the courts of the organisations whose law reaches it.
pub(crate) fn scope_seats(home: &str) -> Vec<String> {
    let order = home.to_ascii_uppercase();
    let path = if order == "FR" {
        vec![
            format!("geo:{}", lj_dtos::DEFAULT_READER_PLACE),
            "geo:FR".to_string(),
        ]
    } else {
        vec![format!("geo:{order}")]
    };
    let courts = lj_norm::journal_rules::court_orders(&path);
    std::iter::once(order)
        .chain(courts.into_iter().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seats(home: &str) -> std::collections::BTreeSet<String> {
        scope_seats(home).into_iter().collect()
    }

    fn set(v: &[&str]) -> std::collections::BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// France reads the Union's courts and the CEDH, Switzerland the CEDH and not
    /// the Union's, Senegal its three West African organisations and the African
    /// Union's court, Morocco, outside its Protocol, its own courts alone.
    #[test]
    fn a_jurisdiction_reads_the_courts_of_the_organisations_it_belongs_to() {
        assert_eq!(seats("fr"), set(&["FR", "EU", "INTL.CEDH"]));
        assert_eq!(seats("ch"), set(&["CH", "INTL.CEDH"]));
        assert_eq!(seats("be"), set(&["BE", "EU", "INTL.CEDH"]));
        assert_eq!(
            seats("sn"),
            set(&["SN", "OHADA", "UEMOA", "CEDEAO", "UNIONAFRICAINE"])
        );
        assert_eq!(
            seats("cm"),
            set(&["CM", "OHADA", "CEMAC", "UNIONAFRICAINE"])
        );
        assert_eq!(seats("ma"), set(&["MA"]));
    }

    #[test]
    fn a_home_is_two_lower_case_letters() {
        assert!(parse_home("ch").is_ok());
        for bad in ["CH", "che", "c", "c1", ""] {
            assert!(parse_home(bad).is_err(), "{bad}");
        }
    }
}
