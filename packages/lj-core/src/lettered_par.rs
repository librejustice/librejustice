//! « par » a pseudonymiser took for a name and lettered: « K une requête », « K lequel ».

use std::sync::LazyLock;

use regex::Regex;

/// Words only « par » stands before: a letter followed by one is the lettered word.
const SIGNATURES: &[&str] = &[
    "lequel",
    "laquelle",
    "lesquels",
    "lesquelles",
    "ailleurs",
    "suite",
    "conséquent",
];

/// Nouns a letter labels: « section K », « catégorie A », « hépatite C ».
const LABELS: &[&str] = &[
    "catégorie",
    "catégories",
    "classe",
    "classes",
    "section",
    "sections",
    "sous-section",
    "annexe",
    "annexes",
    "zone",
    "zones",
    "type",
    "types",
    "groupe",
    "groupes",
    "lot",
    "lots",
    "bâtiment",
    "bâtiments",
    "bloc",
    "escalier",
    "entrée",
    "hépatite",
    "vitamine",
    "permis",
    "pièce",
    "pièces",
    "niveau",
    "échelon",
    "grade",
    "série",
    "liste",
    "tableau",
    "colonne",
    "case",
    "cadre",
    "rubrique",
    "modèle",
    "formulaire",
    "plan",
    "phase",
    "tranche",
    "étape",
    "option",
    "variante",
    "hypothèse",
    "scénario",
    "appendice",
    "figure",
    "schéma",
    "parcelle",
    "parcelles",
    "îlot",
    "secteur",
    "rang",
    "ligne",
    "autoroute",
    "route",
    "salle",
    "unité",
    "pavillon",
    "aile",
    "porte",
    "quai",
    "terminal",
    "cellule",
    "quartier",
    "division",
    "indice",
    "coefficient",
    "note",
    "mention",
    "lettre",
    "cote",
    "bis",
    "ter",
    "quater",
    "quinquies",
    "sexies",
    "septies",
    "octies",
    "nonies",
    "decies",
];

/// Nouns a roman numeral follows: « titre V », « livre I ».
const ROMAN_LABELS: &[&str] = &["titre", "livre", "chapitre", "tome", "partie"];

/// What follows the letter of an article number: « l'article 39 C du code ».
const ARTICLE_TAILS: &[&str] = &[
    "du",
    "de",
    "des",
    "et",
    "ou",
    "à",
    "au",
    "aux",
    "bis",
    "ter",
    "quater",
    "quinquies",
    "sexies",
    "septies",
    "octies",
    "nonies",
    "decies",
];

static OPENING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:une?|des|deux|trois) (?:requêtes?|mémoires?|décisions?|arrêtés?|jugements?|ordonnances?)\b",
    )
    .unwrap()
});

static LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[\dIVXLC]+[-.)°]+|[-\x{2013}\x{2022}/])$").unwrap());

/// What a sentence opened by « Par » goes on with, and one opened by « A » for « À » never does.
static PAR_OPENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:suite|ailleurs|conséquent|contre|voie|dérogation|ces motifs)\b").unwrap()
});

static PAR_OPENS_ACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:une?|des|deux|trois|quatre|cinq|six|plusieurs|ce|cet|cette|ces|la|les|son|sa|ses|leur|leurs)\s+|l['’]\s*)?(?:(?:même|mêmes|nouveau|nouvel|nouvelle|nouveaux|nouvelles|premier|première|second|seconde|dernier|dernière|présente?s?|seule?)\s+)?(?:requêtes?|mémoires?|décisions?|arrêtés?|arrêts?|jugements?|ordonnances?|courriers?|courriels?|lettres?|délibérations?|actes?|avis|pourvois?|recours|écritures|conclusions|observations|notes?|présentes)\b",
    )
    .unwrap()
});

/// « À ce courrier était joint »: the act a sentence opens on is its subject.
static BEING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(?:est|sont|était|étaient)\b").unwrap());

static PARTICIPLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"é(?:e|es|s)?$").unwrap());

/// `text` with « par » written back where a letter stands for it, `None` when none does.
pub fn restored_par(text: &str) -> Option<String> {
    let letter = par_letter(text)?;
    // A text that kept its lowercase « par » had only the capitalised one lettered.
    let capital_only = words(text).any(|w| w == "par");
    let mut out = String::with_capacity(text.len() + 64);
    let mut last = 0;
    for at in lone_capitals(text).filter(|&at| text.as_bytes()[at] == letter) {
        if let Some(word) = restoration(text, at, letter, capital_only) {
            out.push_str(&text[last..at]);
            out.push_str(word);
            last = at + 1;
        }
    }
    if last == 0 {
        return None;
    }
    out.push_str(&text[last..]);
    Some(out)
}

/// The one letter seen before a word only « par » precedes: twice, or once in a text with no « par ».
fn par_letter(text: &str) -> Option<u8> {
    let mut hits = [0u32; 26];
    for at in lone_capitals(text) {
        let (before, after) = line_around(text, at);
        let Some(rest) = after.strip_prefix(' ') else {
            continue;
        };
        let word: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
        if SIGNATURES.contains(&word.as_str())
            || (before.trim().is_empty() && OPENING.is_match(rest))
        {
            hits[(text.as_bytes()[at] - b'A') as usize] += 1;
        }
    }
    let mut seen = hits.iter().enumerate().filter(|(_, &n)| n > 0);
    let (letter, &n) = seen.next()?;
    if seen.next().is_some() {
        return None;
    }
    (n >= 2 || !words(text).any(|w| w == "par" || w == "Par")).then_some(b'A' + letter as u8)
}

/// What the letter at `at` is written back as, `None` when it stays a letter.
fn restoration(text: &str, at: usize, letter: u8, capital_only: bool) -> Option<&'static str> {
    let (before, after) = line_around(text, at);
    let rest = after.strip_prefix(' ')?;
    let next = rest.split(' ').next()?;
    if !next.chars().next()?.is_alphabetic() || spelled_heading(before, rest) {
        return None;
    }
    if in_capitals(before, after) {
        return rest.starts_with("CES MOTIFS").then_some("PAR");
    }
    let tokens: Vec<&str> = before.split_whitespace().collect();
    let prev = tokens.last().copied();
    if let Some(prev) = prev {
        let lower = prev.to_lowercase();
        if LABELS.contains(&lower.as_str())
            || (ROMAN_LABELS.contains(&lower.as_str()) && b"IVXLCDM".contains(&letter))
        {
            return None;
        }
        let next = next.to_lowercase();
        if prev.bytes().all(|b| b.is_ascii_digit())
            && (ARTICLE_TAILS.contains(&next.as_str())
                || next.starts_with("d'")
                || next.starts_with("d’"))
        {
            return None;
        }
    }
    let opens = match prev {
        None => true,
        Some(prev) => {
            prev.ends_with(['.', '!', '?', '"', '«', '“'])
                || LIST_MARKER.is_match(prev)
                || (tokens.len() == 1 && prev.chars().all(|c| "IVXLC0123456789".contains(c)))
        }
    };
    let after_break = prev.is_some_and(|prev| {
        prev.ends_with(':')
            || (prev == "()"
                && !tokens
                    .len()
                    .checked_sub(2)
                    .is_some_and(|i| PARTICIPLE.is_match(tokens[i])))
    });
    if letter == b'A' && (opens || after_break) && !par_opens(rest) {
        return None;
    }
    Some(if opens || capital_only { "Par" } else { "par" })
}

fn par_opens(rest: &str) -> bool {
    PAR_OPENS.is_match(rest)
        || PAR_OPENS_ACT
            .find(rest)
            .is_some_and(|m| !BEING.is_match(&rest[m.end()..]))
}

/// « D E C I D E », « O R D O N N E »: four single letters in a row or more.
fn spelled_heading(before: &str, rest: &str) -> bool {
    let single = |t: &&str| {
        let mut chars = t.chars();
        chars.next().is_some_and(char::is_uppercase) && chars.next().is_none()
    };
    let left = before
        .strip_suffix(' ')
        .map_or(0, |b| b.rsplit(' ').take_while(single).count());
    let right = rest.split(' ').take_while(single).count();
    left + 1 + right >= 4
}

fn in_capitals(before: &str, after: &str) -> bool {
    let (mut letters, mut upper) = (0usize, 0usize);
    for c in before
        .chars()
        .chain(after.chars())
        .filter(|c| c.is_alphabetic())
    {
        letters += 1;
        upper += usize::from(c.is_uppercase());
    }
    letters > 0 && upper * 5 >= letters * 4
}

/// The line around byte `at`, split on either side of it.
fn line_around(text: &str, at: usize) -> (&str, &str) {
    let start = text[..at].rfind('\n').map_or(0, |p| p + 1);
    let end = text[at..].find('\n').map_or(text.len(), |p| at + p);
    (&text[start..at], &text[at + 1..end])
}

fn joins_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\'' | '’' | '-')
}

/// Byte offsets of the lone capitals A to Z: no word character on either side, no period before.
fn lone_capitals(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.bytes()
        .enumerate()
        .filter(|(_, b)| b.is_ascii_uppercase())
        .map(|(at, _)| at)
        .filter(move |&at| {
            let prev = text[..at].chars().next_back();
            let next = text[at + 1..].chars().next();
            !prev.is_some_and(|c| joins_word(c) || c == '.') && !next.is_some_and(joins_word)
        })
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !joins_word(c))
        .filter(|w| !w.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_par_of_a_lettered_text_comes_back() {
        let text = "Vu la procédure suivante :\n\nK une requête, enregistrée le 10 novembre 2022, M. G E, représenté K Me C, demande au tribunal d'annuler l'arrêté du 18 octobre 2022 K lequel le préfet de l'Essonne lui a fait obligation de quitter le territoire.\n\nK un mémoire en défense, le préfet conclut au rejet en soutenant que les moyens soulevés K M. E ne sont pas fondés. K ailleurs, il est hébergé.";
        assert_eq!(
            restored_par(text).unwrap(),
            "Vu la procédure suivante :\n\nPar une requête, enregistrée le 10 novembre 2022, M. G E, représenté par Me C, demande au tribunal d'annuler l'arrêté du 18 octobre 2022 par lequel le préfet de l'Essonne lui a fait obligation de quitter le territoire.\n\nPar un mémoire en défense, le préfet conclut au rejet en soutenant que les moyens soulevés par M. E ne sont pas fondés. Par ailleurs, il est hébergé."
        );
    }

    #[test]
    fn a_text_whose_par_stand_is_left_alone() {
        let text = "Par une requête, M. A, représenté par Me B, demande l'annulation de l'arrêté par lequel le préfet a refusé son titre. Par ailleurs, la section K du plan est en zone A.";
        assert_eq!(restored_par(text), None);
    }

    #[test]
    fn letters_of_initials_articles_headings_and_labels_stay() {
        let text = "D une requête, M. A demande l'annulation de la décision D laquelle le maire a classé la parcelle en zone D, sur le fondement de l'article L. 761-1 et de l'article 39 D du code général des impôts, au titre de la catégorie D.\nD E C I D E :\nArticle 1er : La requête est rejetée.\nLe président,\nD. Martin";
        assert_eq!(
            restored_par(text).unwrap(),
            "Par une requête, M. A demande l'annulation de la décision par laquelle le maire a classé la parcelle en zone D, sur le fondement de l'article L. 761-1 et de l'article 39 D du code général des impôts, au titre de la catégorie D.\nD E C I D E :\nArticle 1er : La requête est rejetée.\nLe président,\nD. Martin"
        );
    }

    #[test]
    fn a_sentence_opening_on_a_stays_a_when_it_reads_a_grave() {
        let text = "A une requête, M. B demande l'annulation de l'arrêté A lequel le préfet a refusé son titre. A cet égard, il produit des pièces. A titre subsidiaire, il demande un réexamen. A ce courrier était joint un certificat. A suite, la requête est accueillie.";
        assert_eq!(
            restored_par(text).unwrap(),
            "Par une requête, M. B demande l'annulation de l'arrêté par lequel le préfet a refusé son titre. A cet égard, il produit des pièces. A titre subsidiaire, il demande un réexamen. A ce courrier était joint un certificat. Par suite, la requête est accueillie."
        );
    }

    #[test]
    fn a_text_lettering_only_the_capital_gets_the_capital_back() {
        let text = "C un mémoire, le préfet conclut au rejet de la requête présentée par M. A. C suite, le moyen tiré par le requérant de l'erreur est écarté. C ailleurs, la décision est motivée.";
        assert_eq!(
            restored_par(text).unwrap(),
            "Par un mémoire, le préfet conclut au rejet de la requête présentée par M. A. Par suite, le moyen tiré par le requérant de l'erreur est écarté. Par ailleurs, la décision est motivée."
        );
    }

    #[test]
    fn one_sign_is_enough_in_a_text_with_no_par_left() {
        let text = "Vu la procédure suivante :\nA une requête enregistrée le 2 novembre 2022, Mme D F demande au juge des référés de suspendre la décision prise A le préfet.";
        assert_eq!(
            restored_par(text).unwrap(),
            "Vu la procédure suivante :\nPar une requête enregistrée le 2 novembre 2022, Mme D F demande au juge des référés de suspendre la décision prise par le préfet."
        );
    }

    #[test]
    fn a_line_in_capitals_gets_par_back_before_ces_motifs_only() {
        let text = "B une requête, M. A demande l'annulation de la décision B laquelle le maire a refusé le permis.\nB CES MOTIFS, LE TRIBUNAL DECIDE :";
        assert_eq!(
            restored_par(text).unwrap(),
            "Par une requête, M. A demande l'annulation de la décision par laquelle le maire a refusé le permis.\nPAR CES MOTIFS, LE TRIBUNAL DECIDE :"
        );
    }
}
