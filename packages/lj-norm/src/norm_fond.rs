//! A text's fond in the norms catalogue, by its order and first type level, from `data/norm_fonds.tsv`.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::jurisdiction_path::{table_order, table_types};

const NORM_FONDS_TSV: &str = include_str!("../data/norm_fonds.tsv");

/// The row of an order that takes every text no other row of it names.
const ANY_TYPE: &str = "*";

const OTHERS: &str = "autres";

static TABLE: LazyLock<HashMap<(&'static str, &'static str), &'static str>> = LazyLock::new(|| {
    let mut table = HashMap::new();
    for line in NORM_FONDS_TSV.lines().skip(1) {
        let mut cols = line.split('\t');
        let (Some(order), Some(kind), Some(fond), None) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            panic!("norm_fonds.tsv: malformed row {line:?}");
        };
        let previous = table.insert((order, kind), fond);
        assert!(previous.is_none(), "norm_fonds.tsv: {order} {kind} twice");
    }
    table
});

/// A text's fond from its path's second element (`FR.LOI`), `None` for an untyped text.
pub fn fond(jurisdiction: &str, first_type: Option<&str>) -> &'static str {
    let order = table_order(jurisdiction);
    first_type
        .and_then(|path| path.strip_prefix(jurisdiction)?.strip_prefix('.'))
        .and_then(|kind| TABLE.get(&(order, kind)))
        .or_else(|| TABLE.get(&(order, ANY_TYPE)))
        .copied()
        .unwrap_or(OTHERS)
}

/// The first-type paths of `jurisdiction` a fond gathers, and whether untyped texts join them.
pub fn fond_paths(jurisdiction: &str, fond: &str) -> (Vec<String>, bool) {
    let paths = table_types(jurisdiction)
        .into_iter()
        .map(|kind| format!("{jurisdiction}.{kind}"))
        .filter(|path| self::fond(jurisdiction, Some(path)) == fond)
        .collect();
    (paths, self::fond(jurisdiction, None) == fond)
}

/// The norms search's `jurisdiction` filter for a fond: its order when it holds all of it, else its paths.
pub fn fond_search_paths(jurisdiction: &str, fond: &str) -> Vec<String> {
    let (paths, untyped) = fond_paths(jurisdiction, fond);
    let whole = untyped && paths.len() == table_types(jurisdiction).len();
    match whole || paths.is_empty() {
        true => vec![jurisdiction.to_string()],
        false => paths,
    }
}

/// The bare types a fond gathers in any order the table names (`CODE`).
pub fn fond_types(fond: &str) -> Vec<&'static str> {
    let mut orders: Vec<&'static str> = TABLE.keys().map(|(order, _)| *order).collect();
    orders.sort_unstable();
    orders.dedup();
    let mut types: Vec<&'static str> = orders
        .into_iter()
        .flat_map(|order| {
            table_types(order)
                .into_iter()
                .filter(move |kind| self::fond(order, Some(&format!("{order}.{kind}"))) == fond)
        })
        .collect();
    types.sort_unstable();
    types.dedup();
    types
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jurisdiction_path::jurisdiction_paths;
    use crate::referential_labels::NORM_FONDS;

    fn fond_of(jurisdiction: &str, nature: &str) -> &'static str {
        let paths = jurisdiction_paths(jurisdiction, nature);
        fond(jurisdiction, paths.get(1).map(String::as_str))
    }

    #[test]
    fn every_row_names_a_type_its_order_gives_and_a_catalogue_fond() {
        for ((order, kind), fond) in TABLE.iter() {
            assert!(NORM_FONDS.contains(fond), "{order} {kind}: fond {fond}");
            assert!(
                *kind == ANY_TYPE || table_types(order).contains(kind),
                "{order} {kind}: no such type in text_types.tsv"
            );
        }
    }

    #[test]
    fn a_text_takes_the_fond_of_its_first_type_level() {
        assert_eq!(fond_of("FR", "LOI_ORGANIQUE"), "lois");
        assert_eq!(fond_of("FR", "ORDONNANCE"), "lois");
        assert_eq!(fond_of("FR", "FICHE_PRATIQUE"), "fiches-pratiques");
        assert_eq!(fond_of("FR", "TI"), "conventions-collectives");
        assert_eq!(fond_of("FR", "AVIS"), "autres");
        assert_eq!(fond_of("FR", "DEUXIEME"), "autres");
        assert_eq!(fond_of("FR-PF", "LOI"), "lois");
        assert_eq!(fond_of("EU", "REGLEMENT"), "textes-ue");
        assert_eq!(fond_of("INTL", "CODE"), "codes");
        assert_eq!(fond_of("INTL", "AVENANT"), "traites");
        assert_eq!(fond_of("CH", "code_civil"), "codes");
        assert_eq!(fond_of("CH", "LOI"), "lois");
        assert_eq!(fond_of("CH", "ORDONNANCE"), "autres");
    }

    #[test]
    fn a_fond_reads_back_as_the_paths_that_land_in_it() {
        let (paths, untyped) = fond_paths("FR", "lois");
        assert_eq!(paths, ["FR.DECRET_LOI", "FR.LOI", "FR.ORDONNANCE"]);
        assert!(!untyped);
        let (paths, untyped) = fond_paths("FR", "autres");
        assert!(paths.contains(&"FR.AVIS".to_string()));
        assert!(!paths.contains(&"FR.LOI".to_string()));
        assert!(untyped);
        assert!(fond_paths("INTL", "traites").1);
        assert_eq!(fond_types("codes"), ["CODE", "CONSTITUTION"]);
    }

    #[test]
    fn a_fond_holding_its_whole_order_is_searched_by_the_order() {
        assert_eq!(fond_search_paths("FR", "decrets"), ["FR.DECRET"]);
        assert_eq!(
            fond_search_paths("FR", "lois"),
            ["FR.DECRET_LOI", "FR.LOI", "FR.ORDONNANCE"]
        );
        assert_eq!(fond_search_paths("EU", "textes-ue"), ["EU"]);
    }
}
