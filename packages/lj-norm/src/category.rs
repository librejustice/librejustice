//! The category a text of the norms is searched under, read off its order and its
//! nature, and the same partition as filter cells over the two fields. PURE.

/// The categories that explain or discuss the norms, which their texts' pages name.
pub const BESIDE_NORMS: [&str; 3] = [
    "doctrine_administrative",
    "information_publique",
    "travaux_parlementaires",
];

/// The administration's official reading of a norm, invocable under its own regime.
pub const DOCTRINE_ADMIN_NATURES: &[&str] = &[
    "ACPR",
    "BOFIP",
    "BOSS",
    "CIRCULAIRE",
    "CNIL",
    "INSTRUCTION",
    "INSTRUCTIONEURO",
    "LETTREEURO",
    "NOTE",
    "RECOMMANDATION",
    "RESCRIT",
];

/// The administration explaining the law to the public, binding nobody.
pub const PUBLIC_INFO_NATURES: &[&str] = &["FICHE_PRATIQUE"];

/// A parliamentary question and its answers.
pub const PARLIAMENT_NATURES: &[&str] = &["QUESTION_ECRITE", "QUESTION_ORALE"];

/// The category of a nature no rule names, outside the international order.
const DEFAULT: &str = "reglementation";

/// The category of each nature, whatever the order but the international one.
const NATURES: &[(&str, &[&str])] = &[
    (
        "constitution",
        &[
            "CONSTITUTION",
            "LOI_CONSTIT",
            "LOI_CONSTITUTIONNELLE",
            "SENATUS",
        ],
    ),
    (
        "codes",
        &[
            "CODE",
            "CODE_AUTRE",
            "CODE_CIVIL",
            "CODE_COMMERCE",
            "CODE_DIP",
            "CODE_DOUANES",
            "CODE_ETAT_CIVIL",
            "CODE_FAMILLE",
            "CODE_IMPOTS",
            "CODE_NATIONALITE",
            "CODE_OBLIGATIONS",
            "CODE_PENAL",
            "CODE_PERSONNES",
            "CODE_PROCEDURE",
            "CODE_PROCEDURE_CIVILE",
            "CODE_PROCEDURE_PENALE",
            "CODE_TRAVAIL",
        ],
    ),
    (
        "legislation",
        &[
            "ACTE_UNIFORME",
            "DECRET_LOI",
            "DIRECTIVE",
            "DIRECTIVE_EURO",
            "ETAT_CIVIL",
            "LOI",
            "LOI_ORGANIQUE",
            "LOI_PROGRAMME",
            "ORDONNANCE",
            "REGLEMENTEUROPEEN",
        ],
    ),
    (
        "traites",
        &[
            "ACCORD",
            "ACCORD MULTILATÉRAL",
            "AMENDEMENT",
            "ARRANGEMENT",
            "CHARTE",
            "CONVENTION",
            "DECLARATION",
            "DENONCIATION",
            "ECHANGE-LETTRES",
            "ECHANGE_LETTRES",
            "ECHANGE_NOTES",
            "ENTENTE",
            "INSTRUMENT",
            "MEMORANDUM",
            "MESURE",
            "PACTE",
            "PROCES_VERBAL",
            "PROTOCOLE",
            "STATUT",
            "TRAITE",
        ],
    ),
    (
        "conventions_collectives",
        &["ACCORD_FONCTION_PUBLIQUE", "AVENANT", "IDCC", "TI"],
    ),
    ("doctrine_administrative", DOCTRINE_ADMIN_NATURES),
    ("information_publique", PUBLIC_INFO_NATURES),
    ("travaux_parlementaires", PARLIAMENT_NATURES),
];

/// The orders where a nature reads otherwise: a Belgian decree is a law of its region,
/// a Swiss ordinance the government's, a European regulation binds as a directive.
const ORDERS: &[(&str, &str, &[&str])] = &[
    ("BE", "legislation", &["DECRET", "ORDONNANCE"]),
    ("CH", "reglementation", &["ORDONNANCE"]),
    ("EU", "legislation", &["DECISION", "REGLEMENT"]),
    ("EU", "doctrine_administrative", &["LETTRE"]),
    ("FR", "conventions_collectives", &["ACCORD"]),
];

/// The international order: every nature a treaty's, its constitutions and codes aside.
const INTERNATIONAL: &str = "INTL";

/// The category of a text of order `jurisdiction` and nature `nature`, in any case.
pub fn text_category(jurisdiction: &str, nature: &str) -> &'static str {
    let nature = nature.to_uppercase();
    if let Some((_, category, _)) = ORDERS
        .iter()
        .find(|(order, _, natures)| *order == jurisdiction && natures.contains(&nature.as_str()))
    {
        return category;
    }
    let named = NATURES
        .iter()
        .find(|(_, natures)| natures.contains(&nature.as_str()))
        .map(|(category, _)| *category);
    match (jurisdiction == INTERNATIONAL, named) {
        (true, Some(c @ ("constitution" | "codes"))) => c,
        (true, _) => "traites",
        (false, Some(c)) => c,
        (false, None) => DEFAULT,
    }
}

/// A set of field values, kept or ruled out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Members {
    In(Vec<&'static str>),
    NotIn(Vec<&'static str>),
}

impl Members {
    fn holds(&self, value: &str) -> bool {
        match self {
            Members::In(v) => v.contains(&value),
            Members::NotIn(v) => !v.contains(&value),
        }
    }
}

/// The texts of one group of orders whose upper-cased nature is in `natures`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub orders: Members,
    pub natures: Members,
}

impl Cell {
    pub fn holds(&self, jurisdiction: &str, nature: &str) -> bool {
        self.orders.holds(jurisdiction) && self.natures.holds(&nature.to_uppercase())
    }
}

/// The texts of `category` as cells: one per order read apart, one for the others.
pub fn category_cells(category: &str) -> Vec<Cell> {
    let mut apart: Vec<&'static str> = ORDERS.iter().map(|(order, _, _)| *order).collect();
    apart.push(INTERNATIONAL);
    apart.sort_unstable();
    apart.dedup();
    let mut named: Vec<&'static str> = NATURES
        .iter()
        .flat_map(|(_, natures)| natures.iter().copied())
        .collect();
    named.extend(
        ORDERS
            .iter()
            .flat_map(|(_, _, natures)| natures.iter().copied()),
    );
    named.sort_unstable();
    named.dedup();
    let cell = |orders: Members, representative: &'static str| {
        let natures = match text_category(representative, "") == category {
            true => Members::NotIn(
                named
                    .iter()
                    .copied()
                    .filter(|n| text_category(representative, n) != category)
                    .collect(),
            ),
            false => Members::In(
                named
                    .iter()
                    .copied()
                    .filter(|n| text_category(representative, n) == category)
                    .collect(),
            ),
        };
        (natures != Members::In(Vec::new())).then_some(Cell { orders, natures })
    };
    let mut cells: Vec<Cell> = apart
        .iter()
        .filter_map(|order| cell(Members::In(vec![*order]), order))
        .collect();
    cells.extend(cell(Members::NotIn(apart.clone()), "ZZ"));
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nature_reads_by_its_order() {
        for (jurisdiction, nature, category) in [
            ("FR", "DECRET", "reglementation"),
            ("BE", "DECRET", "legislation"),
            ("MG", "decret", "reglementation"),
            ("FR", "ORDONNANCE", "legislation"),
            ("CH", "ordonnance", "reglementation"),
            ("EU", "REGLEMENT", "legislation"),
            ("BE", "REGLEMENT", "reglementation"),
            ("INTL", "REGLEMENT", "traites"),
            ("INTL", "CONSTITUTION", "constitution"),
            ("FR", "ACCORD", "conventions_collectives"),
            ("BE", "ACCORD", "traites"),
            ("FR", "LOI_CONSTIT", "constitution"),
            ("FR", "QUESTION_ECRITE", "travaux_parlementaires"),
            ("FR", "FICHE_PRATIQUE", "information_publique"),
            ("FR", "BOFIP", "doctrine_administrative"),
            ("CH", "ACTE", "reglementation"),
            ("SN", "CODE_PENAL", "codes"),
        ] {
            assert_eq!(
                text_category(jurisdiction, nature),
                category,
                "{jurisdiction} {nature}"
            );
        }
    }

    #[test]
    fn the_cells_of_a_category_hold_exactly_its_texts() {
        let orders = ["FR", "BE", "CH", "EU", "INTL", "DJ", "LU", "ZZ"];
        let mut natures: Vec<&str> = NATURES
            .iter()
            .flat_map(|(_, n)| n.iter().copied())
            .collect();
        natures.extend([
            "DECRET",
            "ARRETE",
            "REGLEMENT",
            "DECISION",
            "LETTRE",
            "ACTE",
            "",
            "INCONNU",
        ]);
        for category in lj_vocab::NORM_CATEGORIES {
            let cells = category_cells(category);
            for order in orders {
                for nature in &natures {
                    assert_eq!(
                        cells.iter().any(|c| c.holds(order, nature)),
                        text_category(order, nature) == category,
                        "{category}: {order} {nature}"
                    );
                }
            }
        }
    }
}
