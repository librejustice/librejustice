//! What a record implies for a cell it leaves empty.
//!
//! One table of derivations, read by the spreadsheet that closes the account and
//! by the bench that scores a reader. Without it the same relation would live in
//! two places and drift: the bench would count a DFP quantity the spreadsheet
//! already took from the victim's rate as a value the reader invented.

use super::{product, Poste, Unite, Victime};

/// The number of points of a permanent deficit is the victim's AIPP rate, never
/// restated in the poste's quantity.
pub fn points_of(v: &Victime, p: &Poste) -> Option<f64> {
    match (p.quantite, p.poste.as_deref()) {
        (None, Some("DFP")) => v.taux_dfp_pct,
        (q, _) => q,
    }
}

/// The sum a poste rests on when the record lets it be read as a plain product,
/// `None` when something else prices it. A survival prorata prices the lifetime
/// award and pays back the fraction actually lived, an aggravation pays a
/// complement over indemnities already acquired, and a capitalised poste rests on
/// an annuity and a coefficient unless it writes that product out in euros of
/// annuity.
pub fn priced_sum(p: &Poste) -> Option<f64> {
    if p.capitalisation.is_some() && p.unite != Some(Unite::EuroDeRente) {
        return None;
    }
    if let Some(pr) = &p.prorata_survie {
        return Some(pr.montant_viager);
    }
    let alloue = p.montant_alloue?;
    Some(alloue + p.indemnites_acquises.iter().sum::<f64>())
}

/// The unit price the product imposes when the sum and the quantity are known.
/// It lands on the referential cell of the permanent deficit as often as the
/// price the court writes out.
pub fn implied_unit_value(v: &Victime, p: &Poste) -> Option<f64> {
    let (sum, points) = (priced_sum(p)?, points_of(v, p)?);
    let divisor = points * p.taux_pct.unwrap_or(100.0) / 100.0;
    (divisor.abs() > f64::EPSILON).then(|| sum / divisor)
}

/// The quantity the product imposes when the sum and the unit price are known.
/// On a poste priced in euros of annuity it is the capitalisation coefficient,
/// which no field carries and which names the table the court applied.
pub fn implied_quantity(p: &Poste) -> Option<f64> {
    let (sum, unit) = (priced_sum(p)?, p.valeur_unitaire?);
    let divisor = unit * p.taux_pct.unwrap_or(100.0) / 100.0;
    (divisor.abs() > f64::EPSILON).then(|| sum / divisor)
}

/// The value a poste imposes on one of its cells without that cell being written.
pub fn implied_poste(v: &Victime, p: &Poste, champ: &str) -> Option<f64> {
    match champ {
        "quantite" if p.poste.as_deref() == Some("DFP") => v.taux_dfp_pct,
        "quantite" => implied_quantity(p),
        "part_victime" if p.part_tiers_payeurs.is_none() => p
            .montant_du
            .or_else(|| implied_poste(v, p, "montant_du"))
            .or(p.montant_alloue),
        "montant_du" if !p.spared() => v
            .owed_pct()
            .zip(p.montant_alloue)
            .map(|(pct, plein)| plein * pct / 100.0),
        "montant_alloue" => {
            product(p.valeur_unitaire, points_of(v, p), p.taux_pct).map(|(x, _, _)| x)
        }
        "valeur_unitaire" => implied_unit_value(v, p),
        _ => None,
    }
}

/// The value a victim imposes on one of its aggregates: the total sums the
/// postes, the net sums the shares that come back to her, and the AIPP rate
/// reads back from the number of points of the DFP.
pub fn implied_victime(v: &Victime, champ: &str) -> Option<f64> {
    match champ {
        "total_alloue" => v.postes.iter().map(|p| p.montant_alloue).sum(),
        "net_victime" => v
            .postes
            .iter()
            .map(|p| p.part_victime.or(implied_poste(v, p, "part_victime")))
            .sum(),
        "creance_tiers_payeurs" => v.postes.iter().map(|p| p.part_tiers_payeurs).sum(),
        "taux_dfp_pct" => v
            .postes
            .iter()
            .find(|p| p.poste.as_deref() == Some("DFP"))
            .and_then(|p| p.quantite),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dfp(alloue: f64) -> Poste {
        Poste {
            poste: Some("DFP".to_string()),
            montant_alloue: Some(alloue),
            valeur_unitaire: Some(4005.0),
            ..Default::default()
        }
    }

    #[test]
    fn the_number_of_points_of_a_dfp_is_the_rate_of_the_victim() {
        let v = Victime {
            taux_dfp_pct: Some(60.0),
            postes: vec![dfp(240300.0)],
            ..Default::default()
        };
        assert_eq!(implied_poste(&v, &v.postes[0], "quantite"), Some(60.0));
        assert_eq!(
            implied_poste(&v, &v.postes[0], "montant_alloue"),
            Some(240300.0)
        );
    }

    #[test]
    fn a_rate_written_on_the_poste_gives_back_the_rate_of_the_victim() {
        let mut p = dfp(240300.0);
        p.quantite = Some(60.0);
        let v = Victime {
            postes: vec![p],
            ..Default::default()
        };
        assert_eq!(implied_victime(&v, "taux_dfp_pct"), Some(60.0));
    }

    #[test]
    fn the_unit_value_reads_back_from_the_sum_and_the_number_of_points() {
        let mut p = dfp(240300.0);
        p.valeur_unitaire = None;
        let v = Victime {
            taux_dfp_pct: Some(60.0),
            postes: vec![p],
            ..Default::default()
        };
        assert_eq!(implied_unit_value(&v, &v.postes[0]), Some(4005.0));
    }

    #[test]
    fn a_lifetime_award_cut_to_the_time_lived_prices_itself_on_the_lifetime_sum() {
        let mut p = dfp(112.84);
        p.valeur_unitaire = None;
        p.prorata_survie = Some(super::super::ProrataSurvie {
            montant_viager: 38250.0,
            euro_de_rente: 56.65,
            duree_ans: 0.1671233,
        });
        let v = Victime {
            taux_dfp_pct: Some(15.0),
            postes: vec![p],
            ..Default::default()
        };
        assert_eq!(implied_unit_value(&v, &v.postes[0]), Some(2550.0));
    }

    #[test]
    fn a_poste_the_court_prices_already_reduced_implies_no_unit_value() {
        let mut p = dfp(120150.0);
        (p.valeur_unitaire, p.montant_du, p.montant_alloue) = (None, p.montant_alloue, None);
        let v = Victime {
            taux_dfp_pct: Some(60.0),
            partage_responsabilite_pct: Some(50.0),
            postes: vec![p],
            ..Default::default()
        };
        assert_eq!(implied_unit_value(&v, &v.postes[0]), None);
        assert_eq!(
            implied_poste(&v, &v.postes[0], "part_victime"),
            Some(120150.0)
        );
    }

    #[test]
    fn a_tierce_personne_priced_in_euros_of_annuity_gives_back_its_coefficient() {
        let p = Poste {
            poste: Some("ATP".to_string()),
            montant_alloue: Some(20882.96),
            valeur_unitaire: Some(720.0),
            unite: Some(Unite::EuroDeRente),
            capitalisation: Some(super::super::Capitalisation {
                viagere: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let v = Victime {
            postes: vec![p],
            ..Default::default()
        };
        let coef = implied_quantity(&v.postes[0]).unwrap();
        assert!((coef - 29.0).abs() < 0.005, "{coef}");
    }

    #[test]
    fn a_capitalised_poste_that_hides_its_annuity_implies_no_coefficient() {
        let p = Poste {
            poste: Some("ATP".to_string()),
            montant_alloue: Some(292459.2),
            valeur_unitaire: Some(20.0),
            unite: Some(Unite::Heure),
            capitalisation: Some(super::super::Capitalisation {
                viagere: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let v = Victime {
            postes: vec![p],
            ..Default::default()
        };
        assert_eq!(implied_quantity(&v.postes[0]), None);
    }

    #[test]
    fn the_part_of_the_victim_is_the_whole_poste_when_no_third_party_pays() {
        let v = Victime {
            postes: vec![dfp(240300.0)],
            ..Default::default()
        };
        assert_eq!(
            implied_poste(&v, &v.postes[0], "part_victime"),
            Some(240300.0)
        );
        assert_eq!(implied_victime(&v, "total_alloue"), Some(240300.0));
        assert_eq!(implied_victime(&v, "net_victime"), Some(240300.0));
    }

    #[test]
    fn the_part_of_a_victim_at_fault_is_what_is_owed_not_the_full_poste() {
        let v = Victime {
            partage_responsabilite_pct: Some(50.0),
            postes: vec![dfp(240300.0)],
            ..Default::default()
        };
        assert_eq!(
            implied_poste(&v, &v.postes[0], "part_victime"),
            Some(120150.0)
        );
        assert_eq!(implied_victime(&v, "net_victime"), Some(120150.0));
    }
}
