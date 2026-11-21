//! A Walloon act read on Wallex: its title as cited, its nature, a version that only repeals.

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

const HEADS: [&str; 10] = [
    "Arrêté du Gouvernement wallon",
    "Arrêté de l'Exécutif régional wallon",
    "Arrêté ministériel",
    "Décret-programme",
    "Décret",
    "Accord de coopération",
    "Circulaire ministérielle",
    "Circulaire",
    "Règlement",
    "Arrêté",
];

/// `Décret du 5 février 1998 relatif …`: the date after the act type, a code keeping its name.
pub(super) fn title(heading: &str, adopted: Option<NaiveDate>) -> String {
    super::belgium::dated_title(&HEADS, heading, adopted)
}

/// A page parsed, the articles it titles `Art. null.` and leaves out reported.
pub(super) fn page(html: &str, url: &str) -> anyhow::Result<lj_parse::WallexPage> {
    let page = lj_parse::wallex_page(html).map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    if page.unnumbered > 0 {
        tracing::warn!(
            url,
            unnumbered = page.unnumbered,
            "articles with no number are left out"
        );
    }
    Ok(page)
}

static NOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[-\x{2013}]\s*(?:Décret|Arrêté|Loi|Ordonnance)[^()]*").unwrap());

/// A version that says only that its article is gone: `((...) - Décret du 14 mars 2024, art.30)`.
pub(super) fn repealed(texte: &str) -> bool {
    let bare = NOTE.replace_all(texte, "");
    (bare.contains("...") || bare.contains('…'))
        && bare
            .chars()
            .all(|c| matches!(c, '(' | ')' | '[' | ']' | '.' | '…' | ' ' | '\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_dates_its_act_type_and_a_code_keeps_its_name() {
        let day = NaiveDate::from_ymd_opt(1999, 11, 1);
        assert_eq!(
            title(
                "Arrêté du Gouvernement wallon désignant les représentants",
                day
            ),
            "Arrêté du Gouvernement wallon du 1er novembre 1999 désignant les représentants"
        );
        assert_eq!(
            title("Décret-programme portant des mesures diverses", day),
            "Décret-programme du 1er novembre 1999 portant des mesures diverses"
        );
        assert_eq!(
            title(
                "Code de la démocratie locale et de la décentralisation (CDLD)",
                day
            ),
            "Code de la démocratie locale et de la décentralisation (CDLD)"
        );
        assert_eq!(
            super::super::belgium::nature("Décret-programme du 1er novembre 1999"),
            "DECRET"
        );
        assert!(repealed("((...) - Décret du 14 mars 2024, art.30)"));
        assert!(repealed(
            "(... ) \u{2013} Décret-programme du 26 mars 2026, art. 268)."
        ));
        assert!(!repealed("Le Gouvernement détermine (...) les noms."));
    }
}
