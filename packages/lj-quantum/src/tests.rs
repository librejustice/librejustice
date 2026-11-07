use super::arbitration::{arbitrate, resolution, typo};
use super::observation::{observations as project_all, paf_category, Observation};
use super::spreadsheet::{compute, validate, Proof};
use super::{
    decimals, product, product_tolerance, Composante, Lien, NatureAide, Poste, ProrataSurvie,
    Quantum, Regime, Rente, Tranche, Unite, Victime,
};

fn head(name: &str) -> Poste {
    Poste {
        poste: Some(name.to_string()),
        ..Default::default()
    }
}

fn victim(postes: Vec<Poste>) -> Victime {
    Victime {
        cle: "v1".into(),
        role: "directe".into(),
        partage_responsabilite_pct: Some(100.0),
        postes,
        ..Default::default()
    }
}

fn record(v: Victime) -> Quantum {
    Quantum {
        est_liquidation: true,
        victimes: vec![v],
        ..Default::default()
    }
}

fn observations(q: &Quantum) -> Vec<Observation> {
    project_all(q, &compute(q))
}

#[test]
fn a_three_factor_product_keeps_the_full_rate() {
    // 25 euros le jour plein, 25 % de déficit, 198 jours : 1 237,50. Le tarif ne
    // devient jamais 6,25, sinon la médiane mélange prorata et tarif.
    let (v, src, _) = product(Some(25.0), Some(198.0), Some(25.0)).expect("les trois facteurs");
    assert!((v - 1237.50).abs() < 0.005);
    assert!(src.contains("25 %"), "la source dit le taux : {src}");
}

#[test]
fn a_product_tolerance_follows_the_decimals_of_its_factors() {
    assert_eq!(decimals(1872.0), 0);
    assert_eq!(decimals(35.597), 3);
    assert_eq!(decimals(31.90), 1);
    // Un prix de l'euro de rente à trois décimales fait tolérer 0,94 euro sur ce
    // produit, non un centime.
    let t = product_tolerance(&[1872.0, 35.597]);
    assert!((t - 0.941).abs() < 0.01, "tolérance {t}");
    assert!((product_tolerance(&[2035.0, 9.0]) - 0.005).abs() < 1e-9);
}

#[test]
fn the_dfp_point_multiplies_by_the_victim_rate() {
    // Le nombre de points ne vit qu'une fois dans le record : le head n'en
    // porte pas la quantité.
    let mut v = victim(vec![Poste {
        valeur_unitaire: Some(2035.0),
        ..head("DFP")
    }]);
    v.taux_dfp_pct = Some(9.0);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p0.plein"), Some(18315.0));
}

#[test]
fn the_components_sum_to_the_head_to_the_cent() {
    // Quatre tranches d'un déficit temporaire, dont une proratisée sans que la
    // cour en écrive le sous-total : 125 + 175 + 1 237,50 + 1 742,50.
    let comp = |m: f64| Composante {
        montant: Some(m),
        ..Default::default()
    };
    let v = victim(vec![Poste {
        composantes: vec![
            comp(125.0),
            comp(175.0),
            Composante {
                valeur_unitaire: Some(25.0),
                quantite: Some(198.0),
                taux_pct: Some(25.0),
                ..Default::default()
            },
            comp(1742.50),
        ],
        ..head("DFT")
    }]);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p0.plein"), Some(3280.0));
}

#[test]
fn the_shares_sum_to_the_head_reduced_by_the_split() {
    // Récapitulatif du référentiel : un head évalué 1 100 sous un partage de
    // 3/4 met 825 à la charge du responsable, 200 à la victim et 625 aux tiers
    // payeurs. Les parts somment donc 825, jamais 1 100.
    let mut v = victim(vec![Poste {
        montant_alloue: Some(1100.0),
        part_victime: Some(200.0),
        part_tiers_payeurs: Some(625.0),
        ..head("DSA")
    }]);
    v.partage_responsabilite_pct = Some(75.0);
    let t = compute(&record(v.clone()));
    assert_eq!(t.get("v0.p0.du"), Some(825.0));
    assert!(validate(&record(v)).holds());
}

#[test]
fn the_amount_owed_by_the_liable_party_confirms_the_split() {
    let poste_reduit = |du: f64| {
        let mut v = victim(vec![Poste {
            montant_alloue: Some(3690.0),
            montant_du: Some(du),
            ..head("FD")
        }]);
        v.partage_responsabilite_pct = Some(50.0);
        record(v)
    };
    // Read, the owed amount is a second path beside the head and the rate: it
    // confirms the cell, and a wrong figure localises on it.
    let t = compute(&poste_reduit(1845.0));
    assert_eq!(t.get("v0.p0.du"), Some(1845.0));
    assert_eq!(t.source("v0.p0.du"), Some("lu"));
    let verdict = t.verdict();
    assert!(verdict.holds());
    assert!(verdict.confirmed >= 1, "{verdict:?}");

    let verdict = validate(&poste_reduit(1800.0));
    assert!(
        verdict.conflicts.iter().any(|c| c.cell == "v0.p0.du"),
        "la contradiction se localise sur la case : {:?}",
        verdict.conflicts
    );
}

#[test]
fn shares_summing_to_the_full_head_contradict_the_split() {
    let mut v = victim(vec![Poste {
        montant_alloue: Some(1100.0),
        part_victime: Some(475.0),
        part_tiers_payeurs: Some(625.0),
        ..head("DSA")
    }]);
    v.partage_responsabilite_pct = Some(75.0);
    let verdict = validate(&record(v));
    assert!(!verdict.holds());
    assert!(
        verdict.conflicts.iter().any(|c| c.cell == "v0.p0.du"),
        "la contradiction se localise sur la case : {:?}",
        verdict.conflicts
    );
}

#[test]
fn two_agreeing_paths_confirm_the_cell() {
    let v = victim(vec![Poste {
        montant_alloue: Some(18315.0),
        valeur_unitaire: Some(2035.0),
        quantite: Some(9.0),
        ..head("DFP")
    }]);
    let verdict = validate(&record(v));
    assert!(verdict.holds());
    assert!(verdict.confirmed >= 1, "{verdict:?}");
}

#[test]
fn a_wrong_read_figure_localises_on_its_cell() {
    let v = victim(vec![Poste {
        montant_alloue: Some(18000.0),
        valeur_unitaire: Some(2035.0),
        quantite: Some(9.0),
        ..head("DFP")
    }]);
    let verdict = validate(&record(v));
    let c = verdict
        .conflicts
        .iter()
        .find(|c| c.cell == "v0.p0.plein")
        .expect("la contradiction porte sur le poste");
    assert_eq!(c.first, 18000.0);
    assert_eq!(c.second, 18315.0);
}

#[test]
fn a_component_paid_as_an_annuity_interrupts_the_sum() {
    // Sommer un découpage incomplet accuserait la lecture de l'écart qu'elle
    // vient de déclarer.
    let v = victim(vec![Poste {
        composantes: vec![
            Composante {
                montant: Some(1000.0),
                ..Default::default()
            },
            Composante {
                rente: Some(Rente {
                    montant: Some(500.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
        ..head("ATP")
    }]);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p0.plein"), None);
}

#[test]
fn a_rejected_head_leaves_no_share_to_anyone() {
    let v = victim(vec![Poste {
        montant_alloue: Some(0.0),
        ..head("PA")
    }]);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p0.pv"), Some(0.0));
    assert_eq!(t.get("v0.p0.pt"), Some(0.0));
}

#[test]
fn the_missing_term_of_a_sum_is_deduced() {
    let mut v = victim(vec![
        Poste {
            montant_alloue: Some(1000.0),
            ..head("DSA")
        },
        head("DFT"),
    ]);
    v.total_alloue = Some(2500.0);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p1.plein"), Some(1500.0));
}

#[test]
fn a_cell_alone_in_its_corner_is_verified_by_nothing() {
    let v = victim(vec![Poste {
        montant_alloue: Some(1000.0),
        ..head("PEX")
    }]);
    let t = compute(&record(v));
    // Sa répartition entre parts, et la somme des postes de la victim.
    assert_eq!(t.constraints("v0.p0.plein"), 2);
    assert_eq!(t.constraints("v0.p0.inconnue"), 0);
}

#[test]
fn a_dfp_without_quantity_takes_the_aipp_rate_as_its_points() {
    let mut v = victim(vec![Poste {
        montant_alloue: Some(59400.0),
        valeur_unitaire: Some(1080.0),
        ..head("DFP")
    }]);
    v.taux_dfp_pct = Some(55.0);
    let obs = observations(&record(v));
    assert_eq!(obs[0].quantite, Some(55.0));
    assert_eq!(obs[0].unite.as_deref(), Some("point"));
    assert_eq!(obs[0].axe.as_deref(), Some("grille"));
    assert_eq!(obs[0].hors_table, None);
}

#[test]
fn two_components_at_two_prices_are_two_tariffs_and_one_award() {
    let obs = observations(&record(victim(vec![Poste {
        montant_alloue: Some(154_760.0),
        composantes: vec![
            Composante {
                montant: Some(36_500.0),
                valeur_unitaire: Some(20.0),
                quantite: Some(1825.0),
                unite: Some(Unite::Heure),
                nature_aide: Some(NatureAide::Active),
                ..Default::default()
            },
            Composante {
                montant: Some(118_260.0),
                valeur_unitaire: Some(12.0),
                quantite: Some(9855.0),
                unite: Some(Unite::Heure),
                nature_aide: Some(NatureAide::PresenceResponsable),
                nuit: Some(true),
                ..Default::default()
            },
        ],
        ..head("ATP")
    }])));
    assert_eq!(obs.len(), 3);
    assert_eq!(obs[0].montant, Some(154_760.0));
    assert_eq!(obs[0].valeur_unitaire, None);
    assert_eq!(
        obs[0].hors_table.as_deref(),
        Some("ni tarif ni cotation : pas d'abscisse")
    );
    assert_eq!(obs[1].composante_ord, Some(0));
    assert_eq!(obs[1].valeur_unitaire, Some(20.0));
    assert_eq!(obs[1].nature_aide.as_deref(), Some("active"));
    assert_eq!(obs[1].axe.as_deref(), Some("unite"));
    assert_eq!(obs[1].montant, None);
    assert_eq!(obs[2].valeur_unitaire, Some(12.0));
    assert_eq!(obs[2].nature_aide.as_deref(), Some("presence_responsable"));
    assert_eq!(obs[2].nuit, Some(true));
}

#[test]
fn components_that_only_prorate_the_tariff_of_their_head_add_no_observation() {
    // Un DFT à 30 euros le jour découpé en trois taux écrit le même prix quatre
    // fois ; le compter quatre fois mettrait quatre copies dans la médiane.
    let part = |taux: f64, jours: f64| Composante {
        valeur_unitaire: Some(30.0),
        quantite: Some(jours),
        unite: Some(Unite::Jour),
        taux_pct: Some(taux),
        ..Default::default()
    };
    let obs = observations(&record(victim(vec![Poste {
        montant_alloue: Some(4425.0),
        valeur_unitaire: Some(30.0),
        unite: Some(Unite::Jour),
        composantes: vec![part(100.0, 90.0), part(50.0, 60.0), part(25.0, 100.0)],
        ..head("DFT")
    }])));
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].valeur_unitaire, Some(30.0));
    assert_eq!(obs[0].axe.as_deref(), Some("unite"));
}

#[test]
fn an_hourly_rate_does_not_go_through_the_grid() {
    let obs = observations(&record(victim(vec![Poste {
        montant_alloue: Some(123695.09),
        valeur_unitaire: Some(31.90),
        unite: Some(Unite::Heure),
        ..head("ATP")
    }])));
    assert_eq!(obs[0].axe.as_deref(), Some("unite"));
    assert_eq!(obs[0].unite.as_deref(), Some("heure"));
}

#[test]
fn an_hourly_tariff_without_the_nature_of_the_aid_enters_no_cote() {
    let row = |nature_aide| {
        observations(&record(victim(vec![Poste {
            montant_alloue: Some(2000.0),
            valeur_unitaire: Some(20.0),
            quantite: Some(100.0),
            unite: Some(Unite::Heure),
            nature_aide,
            ..head("FD")
        }])))
        .remove(0)
    };
    assert_eq!(row(Some(NatureAide::Active)).hors_table, None);
    let bare = row(None);
    assert_eq!(bare.axe.as_deref(), Some("unite"));
    assert_eq!(
        bare.hors_table.as_deref(),
        Some("tarif horaire sans la nature de l'aide")
    );
}

#[test]
fn a_poste_outside_the_nomenclature_enters_no_cote() {
    let o = observations(&record(victim(vec![Poste {
        montant_alloue: Some(1200.0),
        valeur_unitaire: Some(100.0),
        quantite: Some(12.0),
        unite: Some(Unite::Mois),
        ..head("HORS_NOMENCLATURE")
    }])))
    .remove(0);
    assert_eq!(o.axe.as_deref(), Some("unite"));
    assert_eq!(o.hors_table.as_deref(), Some("poste hors nomenclature"));
}

#[test]
fn a_head_priced_after_reduction_leaves_the_table_with_its_percentage() {
    let mut v = victim(vec![Poste {
        montant_du: Some(5000.0),
        cotation: Some(3.0),
        ..head("SE")
    }]);
    v.partage_responsabilite_pct = Some(75.0);
    let obs = observations(&record(v));
    assert_eq!(obs[0].axe.as_deref(), Some("cotation"));
    assert_eq!(
        obs[0].hors_table.as_deref(),
        Some("poste chiffré réduit à 75 %")
    );
}

#[test]
fn a_head_in_another_currency_is_converted_at_the_fixed_parity() {
    let mut q = record(victim(vec![Poste {
        montant_alloue: Some(2_400_000.0),
        cotation: Some(3.5),
        ..head("SE")
    }]));
    q.devise = Some("XPF".into());
    let obs = observations(&q);
    assert_eq!(obs[0].axe.as_deref(), Some("cotation"));
    assert_eq!(obs[0].hors_table, None);
    let montant = obs[0].montant.unwrap();
    assert!((montant - 20_112.0).abs() < 0.01, "{montant}");
}

#[test]
fn a_lifetime_award_reduced_to_the_time_lived_closes_and_enters_for_life() {
    let q = record(victim(vec![Poste {
        montant_alloue: Some(939.40),
        cotation: Some(4.0),
        prorata_survie: Some(ProrataSurvie {
            montant_viager: 20_000.0,
            euro_de_rente: 25.761,
            duree_ans: 1.21,
        }),
        ..head("PEP")
    }]));
    let verdict = validate(&q);
    assert!(verdict.holds(), "{verdict:?}");
    assert!(verdict.confirmed >= 1, "{verdict:?}");
    let obs = observations(&q);
    assert_eq!(obs[0].montant, Some(20_000.0));
    assert_eq!(obs[0].hors_table, None);
}

#[test]
fn without_an_established_split_no_line_is_comparable() {
    let mut v = victim(vec![Poste {
        montant_alloue: Some(5000.0),
        cotation: Some(3.0),
        ..head("SE")
    }]);
    v.partage_responsabilite_pct = None;
    let obs = observations(&record(v));
    assert_eq!(
        obs[0].hors_table.as_deref(),
        Some("base des montants non établie")
    );
}

#[test]
fn a_contract_or_a_death_takes_the_amount_off_the_scale() {
    let dfp = |vu: Option<f64>| {
        let mut v = victim(vec![Poste {
            montant_alloue: Some(20000.0),
            valeur_unitaire: vu,
            ..head("DFP")
        }]);
        v.taux_dfp_pct = Some(10.0);
        v.age_consolidation = Some(40.0);
        record(v)
    };
    let reason = |q: &Quantum| observations(q).remove(0).hors_table;
    let mut q = dfp(None);
    q.deces = Some(true);
    assert_eq!(
        reason(&q).as_deref(),
        Some("décès : le montant du DFP ne court que jusqu'au décès")
    );
    let mut q = dfp(Some(2000.0));
    q.deces = Some(true);
    assert_eq!(
        reason(&q),
        None,
        "a point value the court writes stays the scale's"
    );
    let mut q = dfp(Some(2000.0));
    q.regimes = vec![Regime::Gav];
    assert_eq!(
        reason(&q).as_deref(),
        Some("garantie accidents de la vie : le contrat fixe le montant")
    );
}

#[test]
fn a_decision_valued_under_a_foreign_law_is_off_the_scale() {
    let reason = |regime: &str| {
        let mut q = record(victim(vec![Poste {
            montant_alloue: Some(4000.0),
            cotation: Some(2.0),
            ..head("SE")
        }]));
        q.regime_libelle = Some(regime.into());
        observations(&q).remove(0).hors_table
    };
    for regime in [
        "Loi espagnole n°30/1995 du 8 novembre 1995 et décret royal législatif n°8/2004",
        "Responsabilité civile de droit belge applicable à l’accident de la circulation",
        "loi interne italienne applicable en vertu de l'article 3 de la Convention de La Haye",
    ] {
        assert_eq!(
            reason(regime).as_deref(),
            Some("loi étrangère : la cour chiffre selon un autre droit"),
            "{regime}"
        );
    }
    for regime in [
        "loi du 5 juillet 1985 relative aux accidents de la circulation",
        "Responsabilité de l’État français pour faute des encadrants (articles 1240 et suivants du code civil) et responsabilité du conducteur selon le droit anglais applicable",
        "Responsabilité contractuelle de l’organisateur de voyages ; dommages-intérêts des victimes indirectes rejetés sous le droit dominicain",
    ] {
        assert_eq!(reason(regime), None, "{regime}");
    }
}

#[test]
fn a_capitalised_amount_a_lost_income_rate_and_a_survivor_accompaniment_compare_nothing() {
    let reason = |q: &Quantum| observations(q).remove(0).hors_table;
    let priced = |code, price, quantity, unite| {
        record(victim(vec![Poste {
            montant_alloue: Some(price * quantity),
            valeur_unitaire: Some(price),
            quantite: Some(quantity),
            unite: Some(unite),
            ..head(code)
        }]))
    };
    assert_eq!(
        reason(&priced("DSF", 1000.0, 40.0, Unite::EuroDeRente)).as_deref(),
        Some("montant annuel capitalisé : propre à la victime, il ne se compare pas")
    );
    assert_eq!(
        reason(&priced("PGPA", 1500.0, 12.0, Unite::Mois)).as_deref(),
        Some("perte de revenus au jour, au mois ou à l'an : salaire, indemnités journalières et net s'y confondent")
    );
    let spouse = |deces| {
        let mut q = record(Victime {
            role: "indirecte".into(),
            lien: Some(Lien::Conjoint),
            ..victim(vec![Poste {
                montant_alloue: Some(5000.0),
                ..head("PAC")
            }])
        });
        q.deces = deces;
        q
    };
    assert_eq!(reason(&spouse(Some(true))), None);
    assert_eq!(
        reason(&spouse(Some(false))).as_deref(),
        Some("préjudice d'accompagnement d'une victime qui survit : la nomenclature le réserve au décès")
    );
}

#[test]
fn a_rejected_head_leaves_the_table_without_being_a_fault() {
    let obs = observations(&record(victim(vec![Poste {
        montant_alloue: Some(0.0),
        ..head("PEX")
    }])));
    assert_eq!(obs[0].axe, None);
    assert_eq!(
        obs[0].hors_table.as_deref(),
        Some("poste rejeté : la cour n'alloue rien")
    );
}

#[test]
fn the_affection_damage_is_axed_on_the_split_link() {
    let proche = |lien: Lien, mineur, coh| Victime {
        cle: "v1".into(),
        role: "indirecte".into(),
        lien: Some(lien),
        mineur,
        cohabitation: coh,
        partage_responsabilite_pct: Some(100.0),
        postes: vec![Poste {
            montant_alloue: Some(20000.0),
            ..head("PAF")
        }],
        ..Default::default()
    };
    // Le barème du préjudice d'affection est celui du décès : sans l'issue de la
    // victim directe, la catégorie ne se rattache à aucune fourchette publiée.
    let cat = |v: Victime| {
        let mut q = record(v);
        q.deces = Some(true);
        let o = observations(&q).remove(0);
        (o.axe, o.categorie, o.categorie_manque)
    };
    assert_eq!(
        cat(proche(Lien::Enfant, Some(false), Some(true))),
        (
            Some("categorie".into()),
            Some("enfant majeur au foyer".into()),
            None
        )
    );
    // La modalité inconnue remonte la ligne au grain plus grossier, en le disant :
    // la faire disparaître amputerait la mesure de ce qu'elle mesure le mieux.
    assert_eq!(
        cat(proche(Lien::Enfant, Some(false), None)),
        (
            Some("categorie".into()),
            Some("enfant majeur".into()),
            Some("cohabitation".into())
        )
    );
    assert_eq!(
        cat(proche(Lien::Fratrie, None, None)),
        (
            Some("categorie".into()),
            Some("fratrie".into()),
            Some("cohabitation".into())
        )
    );
    assert_eq!(
        cat(proche(Lien::Conjoint, None, None)),
        (Some("categorie".into()), Some("conjoint".into()), None)
    );
}

#[test]
fn the_age_at_the_event_decides_minority_when_the_field_is_missing() {
    let enfant = Victime {
        role: "indirecte".into(),
        lien: Some(Lien::Enfant),
        age_fait: Some(9.0),
        ..Default::default()
    };
    assert_eq!(paf_category(&enfant), ("enfant mineur".into(), None));
}

#[test]
fn an_observation_ordinal_follows_the_record_order() {
    let obs = observations(&record(victim(vec![head("DFP"), head("SE"), head("PEX")])));
    let keys: Vec<_> = obs
        .iter()
        .map(|o| (o.victime_ord, o.poste_ord, o.poste.as_str()))
        .collect();
    assert_eq!(keys, vec![(0, 0, "DFP"), (0, 1, "SE"), (0, 2, "PEX")]);
}

#[test]
fn the_typo_space_holds_only_keying_errors() {
    // 59 600 lu pour 59 400 : un chiffre substitué, donc l'erreur de saisie
    // explique l'écart.
    assert_eq!(
        typo(59600.0, 59400.0).as_deref(),
        Some("6->4 en 3e position")
    );
    // Deux chiffres échangés.
    assert_eq!(typo(1234.0, 2134.0).as_deref(), Some("12 lu 21"));
    // Un écart qu'aucune saisie ne produit : deux chiffres changent.
    assert_eq!(typo(59600.0, 51400.0), None);
}

#[test]
fn the_relation_arity_ranks_the_propositions() {
    // Le produit 2 700 x 22 réclame 59 400 quand le head écrit 59 600 : une
    // relation de deux cofacteurs lus, donc une déduction.
    let mut v = victim(vec![Poste {
        montant_alloue: Some(59600.0),
        valeur_unitaire: Some(2700.0),
        quantite: Some(22.0),
        unite: Some(Unite::Point),
        ..head("DFP")
    }]);
    v.total_alloue = Some(59600.0);
    let r = record(v);
    let t = compute(&r);
    let props = arbitrate(&r, &t);
    let p = props.first().expect("une proposition");
    assert_eq!(p.cell, "v0.p0.plein");
    assert!((p.wanted - 59400.0).abs() < 0.005);
    assert_eq!(p.arity, 2);
    assert!(p.is_deduction());
    assert_eq!(p.typo.as_deref(), Some("6->4 en 3e position"));
}

#[test]
fn a_sixteen_term_sum_designates_no_cell() {
    // Seize postes lus, plus un total que la somme contredit : l'écart peut
    // tenir à n'importe lequel, donc corriger serait un choix déguisé en calcul.
    let postes: Vec<Poste> = (0..16)
        .map(|i| Poste {
            montant_alloue: Some(1000.0),
            ..head(if i == 0 { "DFP" } else { "SE" })
        })
        .collect();
    let mut v = victim(postes);
    v.total_alloue = Some(15000.0);
    let r = record(v);
    let t = compute(&r);
    let props = arbitrate(&r, &t);
    let p = props.first().expect("une proposition");
    assert_eq!(p.cell, "v0.total");
    assert_eq!(p.arity, 17);
    assert!(!p.is_deduction());
}

#[test]
fn without_the_direct_victim_outcome_the_relative_scale_is_unknown() {
    // Le tableau publié est celui du décès, ligne à ligne : « décès de l'autre
    // conjoint », « perte d'un enfant ». Un parent dont l'enfant survit relève
    // d'une population disjointe, et le comparer à cette fourchette n'a pas de
    // sens.
    let parent = Victime {
        role: "indirecte".into(),
        lien: Some(Lien::Parent),
        partage_responsabilite_pct: Some(100.0),
        postes: vec![Poste {
            montant_alloue: Some(3000.0),
            ..head("PAF")
        }],
        ..Default::default()
    };
    let mut q = record(parent);
    let o = observations(&q).remove(0);
    assert_eq!(o.categorie.as_deref(), Some("parent"));
    assert_eq!(o.deces, None);
    assert_eq!(
        o.categorie_manque.as_deref(),
        Some("issue de la victime directe")
    );

    q.deces = Some(false);
    let o = observations(&q).remove(0);
    assert_eq!(o.deces, Some(false));
    assert_eq!(o.categorie_manque, None);
}

#[test]
fn a_relative_carries_the_direct_victim_rate_to_be_reclassed() {
    let directe = Victime {
        role: "directe".into(),
        taux_dfp_pct: Some(90.0),
        partage_responsabilite_pct: Some(100.0),
        postes: vec![Poste {
            montant_alloue: Some(563400.0),
            valeur_unitaire: Some(6260.0),
            quantite: Some(90.0),
            unite: Some(Unite::Point),
            ..head("DFP")
        }],
        ..Default::default()
    };
    let proche = Victime {
        role: "indirecte".into(),
        lien: Some(Lien::Enfant),
        mineur: Some(true),
        partage_responsabilite_pct: Some(100.0),
        postes: vec![Poste {
            montant_alloue: Some(30000.0),
            ..head("PAF")
        }],
        ..Default::default()
    };
    let mut q = record(directe);
    q.deces = Some(false);
    q.regimes = vec![Regime::Badinter];
    q.regime_libelle = Some("loi Badinter".into());
    q.referentiel_cite = Some("référentiel indicatif des cours d'appel".into());
    q.victimes.push(proche);

    let obs = observations(&q);
    let paf = obs.iter().find(|o| o.poste == "PAF").unwrap();
    let c = paf.comparabilite.as_ref().unwrap();
    assert_eq!(c.dfp_victime_directe, Some(90.0));
    assert_eq!(c.regimes, [Regime::Badinter]);
    assert_eq!(c.regime_libelle.as_deref(), Some("loi Badinter"));
    assert_eq!(
        c.referentiel_cite.as_deref(),
        Some("référentiel indicatif des cours d'appel")
    );

    // Une ligne hors table porte déjà sa raison : la reclasser n'a pas d'objet.
    let hors = Victime {
        role: "indirecte".into(),
        lien: Some(Lien::Conjoint),
        partage_responsabilite_pct: Some(100.0),
        postes: vec![Poste {
            montant_alloue: Some(4000.0),
            ..head("FO")
        }],
        ..Default::default()
    };
    q.victimes.push(hors);
    let fo = observations(&q)
        .into_iter()
        .find(|o| o.poste == "FO")
        .unwrap();
    assert!(fo.axe.is_none());
    assert!(fo.comparabilite.is_none());
}

#[test]
fn an_affection_priced_with_another_head_stays_off_the_table() {
    let proche = |libelle: &str| {
        let v = Victime {
            role: "indirecte".into(),
            lien: Some(Lien::Conjoint),
            partage_responsabilite_pct: Some(100.0),
            postes: vec![Poste {
                montant_alloue: Some(20000.0),
                libelle_decision: Some(libelle.into()),
                ..head("PAF")
            }],
            ..Default::default()
        };
        observations(&record(v)).remove(0).hors_table
    };
    assert_eq!(proche("préjudice moral et d'affection"), None);
    for libelle in [
        "préjudice moral et d'accompagnement de fin de vie",
        "préjudice d'affection et troubles dans les conditions d’existence",
    ] {
        assert_eq!(
            proche(libelle).as_deref(),
            Some("préjudice d'affection chiffré avec un autre chef : la somme ne se répartit pas"),
            "{libelle}"
        );
    }
}

#[test]
fn only_heads_indexed_on_the_link_take_the_categorical_axis() {
    let proche = |code: &str, montant: f64| {
        let v = Victime {
            role: "indirecte".into(),
            lien: Some(Lien::Conjoint),
            partage_responsabilite_pct: Some(100.0),
            postes: vec![Poste {
                montant_alloue: Some(montant),
                ..head(code)
            }],
            ..Default::default()
        };
        let mut q = record(v);
        q.deces = Some(true);
        let o = observations(&q).remove(0);
        (o.axe, o.categorie, o.hors_table)
    };
    assert_eq!(
        proche("PAF", 25000.0),
        (Some("categorie".into()), Some("conjoint".into()), None)
    );
    // Une perte de revenus des proches se mesure en revenus perdus, pas au lien :
    // sans abscisse, elle sort de la table en le disant.
    let (axe, cat, hors) = proche("PR", 337781.52);
    assert_eq!((axe, cat), (None, None));
    assert_eq!(
        hors.as_deref(),
        Some("ni tarif ni cotation : pas d'abscisse")
    );
}

#[test]
fn an_isolated_typo_closes_by_a_deduction() {
    let faux = victim(vec![Poste {
        montant_alloue: Some(18000.0),
        valeur_unitaire: Some(2035.0),
        quantite: Some(9.0),
        ..head("DFP")
    }]);
    let q = record(faux);
    let t = compute(&q);
    let chosen = resolution(&q, &t).expect("le produit à deux facteurs désigne la case");
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0].cell, "v0.p0.plein");
    assert_eq!(chosen[0].wanted, 18315.0);
}

#[test]
fn the_contradiction_is_named_by_the_shortest_relation() {
    // Le net faux est réclamé par deux chemins qui tombent sur la même valeur :
    // la somme des cinq parts victim, et l'assiette moins la créance. Le second
    // désigne la case, et l'ordre de propagation ne doit pas décider.
    let mut v = victim(
        ["DSA", "DFT", "SE", "PET", "PA"]
            .iter()
            .enumerate()
            .map(|(i, n)| Poste {
                montant_alloue: Some(1000.0),
                part_victime: Some(if i == 0 { 400.0 } else { 1000.0 }),
                part_tiers_payeurs: Some(if i == 0 { 600.0 } else { 0.0 }),
                ..head(n)
            })
            .collect(),
    );
    v.net_victime = Some(4000.0);
    v.creance_tiers_payeurs = Some(600.0);
    let q = record(v);
    let t = compute(&q);
    let c = t
        .conflicts
        .iter()
        .find(|c| c.cell == "v0.net")
        .expect("le net lu est contredit");
    assert_eq!(t.arity_of(&c.cell, &c.second_source), 3, "{c:?}");

    let chosen = resolution(&q, &t).expect("une relation de trois termes désigne le net");
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0].cell, "v0.net");
    assert_eq!(chosen[0].wanted, 4400.0);
}

#[test]
fn an_award_outside_the_claim_or_the_offer_names_the_crossed_bound() {
    let v = victim(vec![
        Poste {
            montant_alloue: Some(27600.0),
            montant_demande: Some(16000.0),
            ..head("DFP")
        },
        Poste {
            montant_alloue: Some(2232.0),
            montant_offert: Some(3006.44),
            ..head("FD")
        },
        Poste {
            montant_alloue: Some(8000.0),
            montant_demande: Some(9000.0),
            montant_offert: Some(5000.0),
            ..head("SE")
        },
    ]);
    let verdict = validate(&record(v));
    assert!(
        verdict.holds(),
        "a bound posts no cell, so it contradicts none"
    );
    let cells: Vec<&str> = verdict.breaches.iter().map(|b| b.cell.as_str()).collect();
    assert_eq!(cells, ["v0.p0.demande", "v0.p1.offre"]);
    assert_eq!(verdict.breaches[0].award, 27600.0);
}

#[test]
fn the_claim_bounds_what_the_victim_receives_not_the_full_head() {
    // 13 x 1 340 + 10 080 = 27 500 of lost earnings, 8 583,54 of them to the
    // funds: the victim asked 24 211 and receives 18 916,46.
    let v = victim(vec![Poste {
        montant_alloue: Some(27500.0),
        part_victime: Some(18916.46),
        part_tiers_payeurs: Some(8583.54),
        montant_demande: Some(24211.0),
        ..head("PGPA")
    }]);
    assert!(validate(&record(v)).breaches.is_empty());
}

#[test]
fn an_offer_nobody_asked_for_may_be_awarded_in_full() {
    // The relatives ask nothing, the insurer offers 9 700: the court awards it,
    // and the offer is the bound.
    let v = victim(vec![Poste {
        montant_alloue: Some(9700.0),
        montant_demande: Some(0.0),
        montant_offert: Some(9700.0),
        ..head("DFT")
    }]);
    assert!(validate(&record(v)).breaches.is_empty());
}

#[test]
fn a_head_written_only_reduced_is_bounded_on_its_full_value_by_the_offer() {
    let reduced = |offer: f64| {
        let mut v = victim(vec![Poste {
            montant_du: Some(8000.0),
            montant_demande: Some(20000.0),
            montant_offert: Some(offer),
            ..head("DFP")
        }]);
        v.partage_responsabilite_pct = Some(50.0);
        validate(&record(v)).breaches
    };
    assert!(reduced(12000.0).is_empty());
    let crossed = reduced(18000.0);
    assert_eq!(crossed.len(), 1);
    assert_eq!(crossed[0].award, 16000.0);
}

#[test]
fn a_fault_and_a_lost_chance_compound_into_the_rate_owed() {
    let owed = |du: f64| {
        let mut v = victim(vec![Poste {
            montant_alloue: Some(10000.0),
            montant_du: Some(du),
            ..head("SE")
        }]);
        v.partage_responsabilite_pct = Some(50.0);
        v.perte_de_chance_pct = Some(60.0);
        validate(&record(v))
    };
    assert!(owed(3000.0).holds());
    assert!(owed(5000.0).conflicts.iter().any(|c| c.cell == "v0.p0.du"));
    assert!(owed(6000.0).conflicts.iter().any(|c| c.cell == "v0.p0.du"));
}

#[test]
fn a_head_written_only_reduced_gives_back_its_full_value_without_proving_it() {
    let mut v = victim(vec![Poste {
        montant_du: Some(4000.0),
        cotation: Some(4.0),
        ..head("SE")
    }]);
    v.perte_de_chance_pct = Some(25.0);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p0.plein"), Some(16000.0));
    assert_eq!(t.proof("v0.p0.plein"), Some(Proof::Derived));
}

#[test]
fn a_spared_head_owes_its_full_amount_and_the_victim_total_keeps_no_rate() {
    let mut v = victim(vec![
        Poste {
            montant_alloue: Some(90000.0),
            montant_du: Some(27000.0),
            ..head("DFP")
        },
        Poste {
            montant_alloue: Some(1320.0),
            montant_du: Some(1320.0),
            ..head("FD")
        },
    ]);
    v.perte_de_chance_pct = Some(30.0);
    v.total_alloue = Some(91320.0);
    v.assiette_reduite = Some(28320.0);
    assert!(validate(&record(v)).holds());
}

#[test]
fn each_component_owes_its_own_share_and_a_spared_one_its_whole() {
    let comp = |montant: Option<f64>, du: f64| Composante {
        montant,
        montant_du: Some(du),
        ..Default::default()
    };
    let mut v = victim(vec![
        Poste {
            composantes: vec![comp(Some(14850.0), 3712.5), comp(None, 33500.0)],
            ..head("FVA")
        },
        Poste {
            composantes: vec![
                Composante {
                    cotation: Some(4.0),
                    ..comp(Some(16000.0), 4000.0)
                },
                comp(Some(2000.0), 2000.0),
            ],
            ..head("SE")
        },
    ]);
    v.perte_de_chance_pct = Some(25.0);
    let t = compute(&record(v));
    assert!(t.verdict().holds(), "{:?}", t.verdict().conflicts);
    assert_eq!(t.get("v0.p0.c1"), Some(134000.0));
    assert_eq!(t.get("v0.p0.plein"), Some(148850.0));
    assert_eq!(t.get("v0.p0.du"), Some(37212.5));
    assert_eq!(t.get("v0.p1.plein"), Some(18000.0));
    assert_eq!(t.get("v0.p1.du"), Some(6000.0));
}

#[test]
fn a_daily_price_the_court_writes_only_in_hours_comes_from_its_bands() {
    // « 208 224 euros (482 jours X 18 euros X 24 heures) »: the court never
    // writes the 432 euros of a day.
    let v = victim(vec![Poste {
        montant_alloue: Some(208224.0),
        quantite: Some(482.0),
        unite: Some(Unite::Jour),
        tranches: vec![Tranche {
            valeur_unitaire: Some(18.0),
            quantite: Some(24.0),
            unite: Some(Unite::Heure),
            nature_aide: Some(NatureAide::Active),
            ..Default::default()
        }],
        ..head("FD")
    }]);
    let t = compute(&record(v));
    assert_eq!(t.get("v0.p0.vu"), Some(432.0));
    assert_eq!(t.proof("v0.p0.plein"), Some(Proof::Confirmed));
}
