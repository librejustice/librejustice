//! A text's path on the jurisdiction axis: its legal order, then its type of act
//! (`FR.LOI`, `FR.LOI.ORGANIQUE`, `EU.DIRECTIVE`, `INTL.TREATY`), from the closed
//! table of each order in `data/text_types.tsv`, in its official publisher's terms.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

const TEXT_TYPES_TSV: &str = include_str!("../data/text_types.tsv");
const TYPE_LABELS_TSV: &str = include_str!("../data/type_labels.tsv");

struct TypeTable {
    orders: HashSet<&'static str>,
    exact: HashMap<(&'static str, &'static str), &'static str>,
    /// `(order, prefix, type)` for the `CODE_*` rows.
    prefixes: Vec<(&'static str, &'static str, &'static str)>,
}

static TABLE: LazyLock<TypeTable> = LazyLock::new(|| {
    let mut table = TypeTable {
        orders: HashSet::new(),
        exact: HashMap::new(),
        prefixes: Vec::new(),
    };
    for line in TEXT_TYPES_TSV.lines().skip(1) {
        let mut cols = line.split('\t');
        let (Some(order), Some(nature), Some(kind), None) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            panic!("text_types.tsv: malformed row {line:?}");
        };
        table.orders.insert(order);
        match nature.strip_suffix('*') {
            Some(prefix) => table.prefixes.push((order, prefix, kind)),
            None => {
                let previous = table.exact.insert((order, nature), kind);
                assert!(previous.is_none(), "text_types.tsv: {order} {nature} twice");
            }
        }
    }
    table
});

static LABELS: LazyLock<HashMap<(&'static str, &'static str), &'static str>> =
    LazyLock::new(|| {
        let mut labels = HashMap::new();
        for line in TYPE_LABELS_TSV.lines().skip(1) {
            let mut cols = line.split('\t');
            let (Some(order), Some(kind), Some(label), None) =
                (cols.next(), cols.next(), cols.next(), cols.next())
            else {
                panic!("type_labels.tsv: malformed row {line:?}");
            };
            let previous = labels.insert((order, kind), label);
            assert!(previous.is_none(), "type_labels.tsv: {order} {kind} twice");
        }
        labels
    });

/// The order whose table types a text: its own, else its parent's (`FR-PF`
/// reads `FR`), else `*`.
pub fn table_order(jurisdiction: &str) -> &str {
    let orders = &TABLE.orders;
    if orders.contains(jurisdiction) {
        return jurisdiction;
    }
    match jurisdiction.split_once('-') {
        Some((parent, _)) if orders.contains(parent) => parent,
        _ => "*",
    }
}

/// The type of a text, dotted when it has levels (`LOI.ORGANIQUE`), `None` when
/// its order's table does not name its nature.
pub fn text_type(jurisdiction: &str, nature: &str) -> Option<&'static str> {
    let order = table_order(jurisdiction);
    let nature = nature.to_uppercase();
    let table = &*TABLE;
    if let Some(kind) = table.exact.get(&(order, nature.as_str())) {
        return Some(kind);
    }
    table
        .prefixes
        .iter()
        .find(|(o, prefix, _)| *o == order && nature.starts_with(prefix))
        .map(|(_, _, kind)| *kind)
}

/// The first level of every type the table of `jurisdiction` gives, sorted.
pub fn table_types(jurisdiction: &str) -> Vec<&'static str> {
    let order = table_order(jurisdiction);
    let table = &*TABLE;
    let mut types: Vec<&'static str> = table
        .exact
        .iter()
        .filter(|((o, _), _)| *o == order)
        .map(|(_, kind)| *kind)
        .chain(
            table
                .prefixes
                .iter()
                .filter(|(o, _, _)| *o == order)
                .map(|(_, _, kind)| *kind),
        )
        .map(|kind| kind.split('.').next().unwrap_or(kind))
        .collect();
    types.sort_unstable();
    types.dedup();
    types
}

/// The French label of a path: its order's name, or its type's label in its
/// order's table (`FR.LOI.ORGANIQUE` → « Loi organique »).
pub fn path_label(path: &str) -> Option<&'static str> {
    match path.split_once('.') {
        None => crate::referential_labels::jurisdiction_label(path),
        Some((jurisdiction, kind)) => LABELS.get(&(table_order(jurisdiction), kind)).copied(),
    }
}

/// The paths a text sits on: its order, then each level of its type under it.
pub fn jurisdiction_paths(jurisdiction: &str, nature: &str) -> Vec<String> {
    let mut paths = vec![jurisdiction.to_string()];
    if let Some(kind) = text_type(jurisdiction, nature) {
        let mut path = jurisdiction.to_string();
        for level in kind.split('.') {
            path = format!("{path}.{level}");
            paths.push(path.clone());
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_whole_and_names_types_in_capitals() {
        let table = &*TABLE;
        let kinds = table
            .exact
            .values()
            .chain(table.prefixes.iter().map(|(_, _, k)| k));
        for kind in kinds {
            assert!(
                kind.split('.').all(|level| !level.is_empty()
                    && level
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')),
                "{kind}"
            );
        }
        assert!(table.orders.contains("*"));
    }

    #[test]
    fn every_level_of_every_type_has_a_label_and_every_label_a_type() {
        let table = &*TABLE;
        let given: HashSet<(&str, &str)> = table
            .exact
            .iter()
            .map(|((order, _), kind)| (*order, *kind))
            .chain(
                table
                    .prefixes
                    .iter()
                    .map(|(order, _, kind)| (*order, *kind)),
            )
            .flat_map(|(order, kind)| {
                kind.match_indices('.')
                    .map(move |(i, _)| (order, &kind[..i]))
                    .chain([(order, kind)])
            })
            .collect();
        for (order, kind) in &given {
            assert!(
                LABELS.contains_key(&(*order, *kind)),
                "{order} {kind}: no label"
            );
        }
        for (order, kind) in LABELS.keys() {
            assert!(
                given.contains(&(*order, *kind)),
                "{order} {kind}: no such type"
            );
        }
    }

    #[test]
    fn a_path_reads_as_its_order_or_its_type() {
        assert_eq!(path_label("FR"), Some("France"));
        assert_eq!(path_label("FR.LOI.ORGANIQUE"), Some("Loi organique"));
        assert_eq!(path_label("FR-PF.CODE"), Some("Code"));
        assert_eq!(path_label("CH.LAW"), Some("Loi"));
        assert_eq!(
            path_label("INTL.EXCHANGE_OF_LETTERS"),
            Some("Échange de lettres")
        );
    }

    #[test]
    fn france_keeps_the_dila_codes_with_the_laws_under_loi() {
        assert_eq!(jurisdiction_paths("FR", "LOI"), ["FR", "FR.LOI"]);
        assert_eq!(
            jurisdiction_paths("FR", "LOI_ORGANIQUE"),
            ["FR", "FR.LOI", "FR.LOI.ORGANIQUE"]
        );
        assert_eq!(jurisdiction_paths("FR", "bofip"), ["FR", "FR.BOFIP"]);
    }

    #[test]
    fn a_nature_outside_its_table_names_no_type() {
        assert_eq!(jurisdiction_paths("FR", ""), ["FR"]);
        assert_eq!(jurisdiction_paths("FR", "DEUXIEME"), ["FR"]);
        assert_eq!(jurisdiction_paths("TD", "RAPPORT"), ["TD"]);
    }

    #[test]
    fn an_order_without_a_table_reads_its_parent_then_the_default() {
        assert_eq!(jurisdiction_paths("FR-PF", "CODE"), ["FR-PF", "FR-PF.CODE"]);
        assert_eq!(jurisdiction_paths("SN", "code_famille"), ["SN", "SN.CODE"]);
        assert_eq!(jurisdiction_paths("CH", "LOI"), ["CH", "CH.LAW"]);
        assert_eq!(jurisdiction_paths("TD", "etat_civil"), ["TD", "TD.LAW"]);
    }

    #[test]
    fn the_union_and_the_treaties_take_english() {
        assert_eq!(
            jurisdiction_paths("EU", "REGLEMENT"),
            ["EU", "EU.REGULATION"]
        );
        assert_eq!(
            jurisdiction_paths("EU", "DIRECTIVE_EURO"),
            ["EU", "EU.DIRECTIVE"]
        );
        assert_eq!(
            jurisdiction_paths("INTL", "TRAITE"),
            ["INTL", "INTL.TREATY"]
        );
        assert_eq!(
            jurisdiction_paths("INTL", "ECHANGE-LETTRES"),
            ["INTL", "INTL.EXCHANGE_OF_LETTERS"]
        );
        assert_eq!(
            jurisdiction_paths("INTL", "AVENANT"),
            ["INTL", "INTL.AMENDMENT"]
        );
    }
}
