//! A joined decision, published once more under each number it joins without its ruling.

use std::sync::LazyLock;

use regex::Regex;

static JOINDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(jonction|joint|jointe|joints|jointes)\b").unwrap());

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{2})\s*[/'\x{2019}-]\s*(\d{1,6})\b").unwrap());

static DOCKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{2})/(\d{1,6})$").unwrap());

static RULING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)par ces motifs|^\s*(ordonne|ordonnons|condamne|condamnons|déclare|declare|déclarons|dit|disons|infirme|confirme|déboute|deboute|déboutons|rejette|rejetons|constate|constatons|renvoie|renvoyons|annule|fixe|fixons)\b",
    )
    .unwrap()
});

const BEFORE: usize = 200;
const AFTER: usize = 250;

/// Whether `text` rules: an operative part, or a line opening on an operative verb.
pub fn holds_ruling(text: &str) -> bool {
    RULING.is_match(text)
}

/// Whether `text` names `docket` (`YY/N`) next to a joinder, exactly or with one digit
/// dropped, as the Aix clerk wrote « 21 ' 1203 » for 21/12103.
pub fn joins(text: &str, docket: &str) -> bool {
    let Some(d) = DOCKET.captures(docket) else {
        return false;
    };
    let (year, seq) = (&d[1], d[2].trim_start_matches('0'));
    let near: Vec<(usize, usize)> = JOINDER
        .find_iter(text)
        .map(|m| (m.start().saturating_sub(BEFORE), m.end() + AFTER))
        .collect();
    NUMBER.captures_iter(text).any(|n| {
        let at = n.get(0).unwrap().start();
        let written = n[2].trim_start_matches('0');
        &n[1] == year
            && near.iter().any(|&(from, to)| (from..=to).contains(&at))
            && (written == seq || drops_one_digit(written, seq))
    })
}

fn drops_one_digit(written: &str, seq: &str) -> bool {
    written.len() + 1 == seq.len()
        && (0..seq.len()).any(|i| {
            seq[..i]
                .chars()
                .chain(seq[i + 1..].chars())
                .eq(written.chars())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_named_under_the_joinder_is_joined() {
        let aix = "Ordonne la jonction des deux procédures enrôlées sous les numéros 21 ' 1472 et 21 ' 1203 sous ce dernier numéro,";
        assert!(joins(aix, "21/12103"));
        assert!(joins(aix, "21/14272"));
        assert!(joins("N° RG 23/00776 / Jonction RG 23/777", "23/00777"));
        assert!(joins(
            "Ordonne la jonction du dossier enregistré sous le numéro 22-00955 à celui figurant sous le numéro de RG 21-21732,",
            "22/00955"
        ));
        assert!(joins(
            "N° RG 23/00640 - N° Portalis DBVD-V-B7H-DSAW / N° RG 23/00647 joint",
            "23/00647"
        ));
    }

    #[test]
    fn another_number_or_a_number_far_from_the_joinder_is_not() {
        let order = "Ordonnons la jonction des instances suivantes : N° RG 24/08591 joint au N° RG 24/04587";
        assert!(!joins(order, "24/08587"));
        assert!(!joins(order, "23/08591"));
        let far = format!(
            "Ordonne la jonction des instances. {} RG 22/00470",
            "x".repeat(300)
        );
        assert!(!joins(&far, "22/00470"));
        assert!(!joins("Le conjoint 22/00470 a fait appel.", "22/00470"));
        assert!(!joins(
            "Joint les pourvois n° 20-11.926 et 20-11.927",
            "20-11.926"
        ));
    }

    #[test]
    fn a_header_holds_no_ruling() {
        let header = "COUR D'APPEL D'AIX-EN-PROVENCE\n\nChambre 1-1\n\nARRÊT AU FOND\n\nRôle N° RG 21/12103\n\nDéclaration d'appel du 6 août 2021\n\nSigné par Monsieur Olivier BRUE, Président";
        assert!(!holds_ruling(header));
        assert!(holds_ruling("MOTIFS\n\nPAR CES MOTIFS\n\nLa cour,"));
        assert!(holds_ruling(
            "Vu les articles 367, 368 et 783 du code de procédure civile,\nOrdonnons la jonction des instances suivantes"
        ));
        assert!(holds_ruling("La cour,\n   DIT n'y avoir lieu à statuer"));
    }
}
