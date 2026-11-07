//! Where the moyens annexés of a Cour de cassation decision start.

use std::sync::LazyLock;

use regex::Regex;

/// « MOYENS ANNEXES » anywhere, « Moyen produit par » heading a line: where nearly
/// every Judilibre record zoning its annexes starts them.
static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)\bMOYENS?[ \t]+(?:ANNEX[EÉ]S?|PRODUITS?)\b|^[ \t]*(Moyens?[ \t]+(?:annex[ée]s?|produits?)\b)",
    )
    .unwrap()
});

/// Codepoint offset of the first annex heading of `text`, `None` without one.
pub fn start(text: &str) -> Option<usize> {
    let caps = HEADING.captures(text)?;
    let at = caps.get(1).unwrap_or_else(|| caps.get(0).unwrap()).start();
    Some(text[..at].chars().count())
}

#[cfg(test)]
mod tests {
    use super::start;

    #[test]
    fn the_heading_opens_the_annexes_wherever_the_court_puts_it() {
        let inline =
            "prononcé le huit janvier deux mille quinze. MOYENS ANNEXES au présent arrêt\n";
        assert_eq!(
            start(inline),
            Some(inline[..inline.find("MOYENS").unwrap()].chars().count())
        );
        let line = "deux mille six.\n\n Moyen produit par la SCP X, avocat aux Conseils\n";
        assert_eq!(start(line), line.find("Moyen "));
        assert_eq!(start("é MOYEN ANNEXÉ"), Some(2));
    }

    #[test]
    fn a_moyen_named_in_the_reasons_is_no_heading() {
        assert_eq!(
            start("Sur le moyen produit par la société, ci-après annexé"),
            None
        );
        assert_eq!(start("PRÉTENTIONS ET MOYENS DES PARTIES"), None);
    }
}
