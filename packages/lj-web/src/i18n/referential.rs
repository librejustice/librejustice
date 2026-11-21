use super::Lang;

/// The labels of the referential's values, by the key the API sends with each, the uid
/// without its facet: `REJET`, `A_TROIS`. `None` keeps the label the API sends, in French.
pub struct ReferentialUi {
    pub solution: fn(&str) -> Option<&'static str>,
    /// A court type, by its seat path, `NL.COUR_APPEL`, else by its second segment, `TA`.
    pub jurisdiction_type: fn(&str) -> Option<&'static str>,
    pub legal_domain: fn(&str) -> Option<&'static str>,
    pub formation: fn(&str) -> Option<&'static str>,
    pub publication: fn(&str) -> Option<&'static str>,
    /// A text's type, by its upper-case code in either vocabulary: `ORDONNANCE`, `ORDINANCE`.
    pub nature: fn(&str) -> Option<&'static str>,
    /// A norms source, by its key: `fedlex`, `lexwork`.
    pub norm_source: fn(&str) -> Option<&'static str>,
    /// A category of the norms, by its key of `lj_vocab::NORM_CATEGORIES`.
    pub norm_scope: fn(&str) -> Option<&'static str>,
    /// A decision's seat, by its parts.
    pub seat: fn(&lj_dtos::SeatParts) -> Option<String>,
}

impl ReferentialUi {
    /// A tag's label from `table`, else the one the API sent.
    pub fn label(table: fn(&str) -> Option<&'static str>, tag: &lj_dtos::FacetTag) -> String {
        table(last_segment(&tag.key)).map_or_else(|| tag.label.clone(), str::to_string)
    }

    /// A seat named from its parts, else the French one the API composed.
    pub fn named_seat(
        &self,
        french: Option<String>,
        parts: Option<&lj_dtos::SeatParts>,
    ) -> Option<String> {
        parts.and_then(self.seat).or(french)
    }
}

/// A decision search's facets, each value named in `lang`: the seat and formation trees by
/// their state and court type, a court by its name, a formation by its last segment, a cited
/// cantonal act by its canton.
pub fn relabel_facets(lang: Lang, mut facets: lj_dtos::SearchFacets) -> lj_dtos::SearchFacets {
    let ui = lang.ui();
    let referential = &ui.referential;
    for choice in facets.seat.iter_mut().chain(facets.formation.iter_mut()) {
        let court = facets
            .seat_names
            .get(&choice.value)
            .and_then(|names| names.get(lang.code()));
        let named = match (court, choice.value.split('.').count()) {
            (Some(name), _) => Some(name.as_str()),
            (None, 1) => (ui.shell.state_name)(&choice.value.to_ascii_lowercase()),
            (None, 2) => (referential.jurisdiction_type)(&choice.value)
                .or_else(|| (referential.jurisdiction_type)(last_segment(&choice.value))),
            (None, _) => (referential.formation)(last_segment(&choice.value)),
        };
        if let Some(label) = named {
            choice.label = label.to_string();
        }
    }
    for (choices, table) in [
        (&mut facets.solution, referential.solution),
        (&mut facets.legal_domain, referential.legal_domain),
        (&mut facets.publication, referential.publication),
    ] {
        for choice in choices.iter_mut() {
            if let Some(label) = table(&choice.value) {
                choice.label = label.to_string();
            }
        }
    }
    for instrument in facets.legal_instrument.iter_mut() {
        super::relabel_title(lang, &mut instrument.label);
    }
    for choice in facets.legal_article_top.iter_mut() {
        super::relabel_title(lang, &mut choice.label);
    }
    facets
}

/// A norms search's facets, each value named in `lang`: a jurisdiction or territory as a place,
/// a path's type, a source, a scope.
pub fn relabel_article_facets(
    lang: Lang,
    mut facets: lj_dtos::ArticleSearchFacets,
) -> lj_dtos::ArticleSearchFacets {
    let referential = &lang.ui().referential;
    for choice in facets.jurisdiction.iter_mut() {
        choice.label = super::place_name(lang, &choice.value, &choice.label);
    }
    for choice in facets.path.iter_mut() {
        let segments: Vec<&str> = choice.value.split('.').collect();
        let named = match segments.as_slice() {
            [order] => Some(super::place_name(lang, order, &choice.label)),
            [order, territory]
                if (lang.ui().shell.place_name)(&format!("{order}-{territory}")).is_some() =>
            {
                Some(super::place_name(
                    lang,
                    &format!("{order}-{territory}"),
                    &choice.label,
                ))
            }
            [order, .., kind] => (referential.nature)(&format!("{order}.{kind}"))
                .or_else(|| (referential.nature)(kind))
                .map(str::to_string),
            [] => None,
        };
        if let Some(label) = named {
            choice.label = label;
        }
    }
    for choice in facets.source.iter_mut() {
        let named = (referential.norm_source)(&choice.value)
            .or_else(|| (referential.nature)(&choice.value.to_ascii_uppercase()));
        if let Some(label) = named {
            choice.label = label.to_string();
        }
    }
    for choice in facets.scope.iter_mut() {
        if let Some(label) = (referential.norm_scope)(&choice.value) {
            choice.label = label.to_string();
        }
    }
    facets
}

/// A formation's key is its seat path, `FR.CC.SECTION`; the other keys have one segment.
fn last_segment(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use crate::i18n::Lang;
    use lj_dtos::{FacetChoice, SearchFacets};

    fn choice(value: &str, label: &str) -> FacetChoice {
        FacetChoice {
            value: value.to_string(),
            label: label.to_string(),
            count: 1,
            parent: None,
            hint: None,
        }
    }

    #[test]
    fn a_seat_tree_is_named_by_state_court_type_and_formation() {
        let facets = SearchFacets {
            seat: vec![
                choice("CH.COUR_APPEL", "Cour d'appel"),
                choice(
                    "CH.COUR_APPEL.BALE_VILLE",
                    "Cour d'appel (Bâle-Ville, Suisse)",
                ),
            ],
            seat_names: [(
                "CH.COUR_APPEL.BALE_VILLE".to_string(),
                [(
                    "de".to_string(),
                    "Appellationsgericht (Basel-Stadt, Schweiz)".to_string(),
                )]
                .into(),
            )]
            .into(),
            formation: vec![
                choice("FR", "France"),
                choice("FR.CC", "Cour de cassation"),
                choice("FR.CC.SECTION", "Formation de section"),
                choice("FR.TA", "Tribunal administratif"),
            ],
            solution: vec![choice("REJET", "Rejet"), choice("NOUVEAU", "Nouveau")],
            legal_article_top: vec![choice(
                "CH-SG/951.1|12",
                "12 · Gesetz über die Verwaltungsrechtspflege (VRP) (Saint-Gall), sGS 951.1",
            )],
            ..Default::default()
        };
        let facets = super::relabel_facets(Lang::De, facets);
        let labels =
            |choices: &[FacetChoice]| choices.iter().map(|c| c.label.clone()).collect::<Vec<_>>();
        assert_eq!(
            labels(&facets.formation),
            [
                "Frankreich",
                "Cour de cassation",
                "Sektion",
                "Verwaltungsgericht"
            ]
        );
        assert_eq!(
            labels(&facets.seat),
            [
                "Berufungsgericht",
                "Appellationsgericht (Basel-Stadt, Schweiz)"
            ]
        );
        assert_eq!(labels(&facets.solution), ["Abweisung", "Nouveau"]);
        assert_eq!(
            labels(&facets.legal_article_top),
            ["12 · Gesetz über die Verwaltungsrechtspflege (VRP) (St. Gallen), sGS 951.1"]
        );
    }

    /// A court type a state names its own way is read by its seat path before its type.
    #[test]
    fn a_court_type_is_named_by_its_state_before_its_type() {
        let facets = SearchFacets {
            seat: vec![
                choice("NL.COUR_APPEL", "Cour d'appel"),
                choice("BE.COUR_APPEL", "Cour d'appel"),
            ],
            ..Default::default()
        };
        let facets = super::relabel_facets(Lang::Nl, facets);
        let labels: Vec<_> = facets.seat.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Gerechtshof", "Hof van beroep"]);
    }

    #[test]
    fn a_norms_path_is_named_by_place_territory_then_type() {
        let facets = lj_dtos::ArticleSearchFacets {
            jurisdiction: vec![choice("CH-VD", "CH-VD")],
            path: vec![
                choice("CH", "Suisse"),
                choice("CH.BS", "Bâle-Ville"),
                choice("CH.BS.ORDINANCE", "Ordonnance"),
                choice("EU.REGULATION", "Règlement"),
                choice("CH.REGULATION", "Règlement"),
            ],
            source: vec![choice("fedlex", "Fedlex (Suisse)"), choice("loi", "loi")],
            code: Vec::new(),
            scope: Vec::new(),
        };
        let facets = super::relabel_article_facets(Lang::De, facets);
        let labels =
            |choices: &[FacetChoice]| choices.iter().map(|c| c.label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(&facets.jurisdiction), ["Waadt"]);
        assert_eq!(
            labels(&facets.path),
            [
                "Schweiz",
                "Basel-Stadt",
                "Verordnung",
                "Verordnung",
                "Reglement"
            ]
        );
        assert_eq!(labels(&facets.source), ["Fedlex (Schweiz)", "Gesetz"]);
    }
}
