//! Chronology steps the corpus does not hold. A `decision_links` row whose
//! target stays pending (`target_decision_id IS NULL`) leaves nothing but its
//! address: it is read back here into a named step, which the page renders
//! without a link. A case the CEDH communicated is named here too.

use crate::referential::Referential;

/// One chronology step read back from a pending address.
pub struct PendingStep {
    pub label: String,
    pub date: String,
    pub docket: Option<String>,
}

/// `None` when the court matches no referential unit, which is what a court
/// the gazetteer could not name looks like, and when the address carries no
/// date, a step with nothing to place it on the chain.
pub fn pending_step(
    court: Option<&str>,
    docket: Option<&str>,
    date: Option<&str>,
    refs: &Referential,
) -> Option<PendingStep> {
    Some(PendingStep {
        label: refs.court_by_key(court?)?.to_string(),
        date: date?.to_string(),
        docket: docket.map(str::to_string),
    })
}

/// The label of a case the CEDH communicated, past two applications the first
/// one and a count.
pub fn communicated_label(appnos: &[String]) -> String {
    match appnos {
        [] => "Requête communiquée par la CEDH".to_string(),
        [one] => format!("Requête n° {one} communiquée par la CEDH"),
        [a, b] => format!("Requêtes nos {a} et {b} communiquées par la CEDH"),
        [first, rest @ ..] => format!(
            "Requêtes nos {first} et {} autres communiquées par la CEDH",
            rest.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_communicated_case_names_one_or_two_applications_then_counts_the_rest() {
        let apps = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            communicated_label(&apps(&["18260/25"])),
            "Requête n° 18260/25 communiquée par la CEDH"
        );
        assert_eq!(
            communicated_label(&apps(&["281/15", "34445/15"])),
            "Requêtes nos 281/15 et 34445/15 communiquées par la CEDH"
        );
        assert_eq!(
            communicated_label(&apps(&["68974/11", "2395/12", "76324/13"])),
            "Requêtes nos 68974/11 et 2 autres communiquées par la CEDH"
        );
    }
    use lj_store::repository::JurisdictionRow;

    fn refs() -> Referential {
        let rows = [
            (
                "ca_aix_en_provence",
                "ca_aix_provence",
                "CA",
                "Cour d'appel d'Aix-en-Provence",
            ),
            (
                "tj_le_havre",
                "tj76351",
                "TJ",
                "Tribunal judiciaire du Havre",
            ),
            (
                "caa_lyon",
                "caa_lyon",
                "CAA",
                "Cour administrative d'appel de Lyon",
            ),
            ("ta_lyon", "ta_lyon", "TA", "Tribunal administratif de Lyon"),
            ("cc", "cc", "CC", "Cour de cassation"),
        ];
        Referential::new(
            Vec::new(),
            rows.iter()
                .map(
                    |(code, source_code, jurisdiction_type, label)| JurisdictionRow {
                        code: (*code).to_string(),
                        source_code: (*source_code).to_string(),
                        jurisdiction_type: (*jurisdiction_type).to_string(),
                        city: None,
                        label: (*label).to_string(),
                        place: None,
                    },
                )
                .collect(),
            Vec::new(),
        )
    }

    /// The court is named by its code, whatever the family the key grammar used
    /// to sort it into, and an address without a docket is a step all the same.
    #[test]
    fn a_court_of_the_referential_names_its_step() {
        let refs = refs();
        let step = |court, docket, date| {
            pending_step(Some(court), docket, Some(date), &refs)
                .map(|s| (s.label, s.date, s.docket))
        };
        assert_eq!(
            step("ta_lyon", Some("0907255"), "2012-03-27"),
            Some((
                "Tribunal administratif de Lyon".to_string(),
                "2012-03-27".to_string(),
                Some("0907255".to_string())
            ))
        );
        assert_eq!(
            step("caa_lyon", Some("02LY02109"), "2003-03-31").map(|s| s.0),
            Some("Cour administrative d'appel de Lyon".to_string())
        );
        assert_eq!(
            step("ca_aix_en_provence", None, "2021-03-03"),
            Some((
                "Cour d'appel d'Aix-en-Provence".to_string(),
                "2021-03-03".to_string(),
                None
            ))
        );
        assert_eq!(
            step("tj_le_havre", Some("24/01880"), "2024-06-30").map(|s| s.0),
            Some("Tribunal judiciaire du Havre".to_string())
        );
    }

    #[test]
    fn an_address_without_court_or_date_yields_no_step() {
        let refs = refs();
        assert!(pending_step(Some("tj_nulle_part"), None, Some("2024-06-30"), &refs).is_none());
        assert!(pending_step(None, Some("0907255"), Some("2012-03-27"), &refs).is_none());
        assert!(pending_step(Some("cc"), Some("04-15.329"), None, &refs).is_none());
    }
}
