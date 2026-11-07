//! A text read off a page, by `pdftotext` or by an OCR, back into paragraphs, one per line.
//! The page width is read from the text, so a text already reflowed must not pass again: its
//! long lines would swallow the line after them.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

static PAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(?:[-\x{2013}]\s*)?\d{1,3}(?:\s*[-\x{2013}])?|\d{1,3}\s*/\s*\d{1,3}|(?:RG\s*n°\s*[\d/]+\s*[-\x{2013}]\s*)?(?:[-\x{2013}]\s*)?(?:page\s*:?\s*[-\x{2013}]?\s*\d+\s*[-\x{2013}]?(?:\s*(?:/|sur|de)\s*\d+)?|\d+\s*(?:ème|e|è)\s+page)(?:\s*[-\x{2013}])?)$",
    )
    .unwrap()
});
static ARTIFACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^The (?:Ground Truth )?image (?:displays|contains|is)\b").unwrap()
});
static RULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:_{3,}|-{4,}|\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)*\|?)$").unwrap()
});
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[-\x{2013}\x{2014}\x{2022}·]\s|\d{1,3}\s*(?:°\s*\)?|\))\s*|\d{1,3}\.\s|[IVX]{1,5}\s*[-.\x{2013})]\s|[a-h]\)\s|\*{3}|Article\s+(?:1er|\d+)\b|Considérant|CONSIDÉRANT|Sur\s|SUR\s|Vu\s|VU\s|Par ces motifs|PAR CES MOTIFS|D\s*[ÉE]\s*C\s*I\s*D\s*E|ORDONNE|Fait à|Délibéré|Lu en|Ainsi (?:jugé|fait)|Rendu)",
    )
    .unwrap()
});
static BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-\x{2013}\x{2014}\x{2022}·]\s").unwrap());
static OPEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\b(?:du|de|des|la|le|les|à|au|aux|et|ou|par|pour|en|sur|dans|avec|contre|entre|que|qui|un|une|son|sa|ses|leur|leurs|ce|cette|ces|ni|dont|sans|sous|vers|chez)|\b[dl]['’])$",
    )
    .unwrap()
});
static ABBREVIATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:M|MM|Mme|Mmes|Mlle|Me|Mes|Dr|Pr|art|al|n°|L|R|D|cf|av|bd|St|Ste)\.$").unwrap()
});
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:(?:DE|DES|DU|ET|EN|À|AUX)\s|D['’]\w)").unwrap());
static CUT_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Za-zÀ-ÿ]+)-$").unwrap());
static ITALIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*(\S(?:[^*\n]{0,198}\S)?)\*").unwrap());
static MD_HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s+").unwrap());
static MD_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
static MD_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)\s]*\)").unwrap());
static LOWER_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)[a-zà-ÿ]{3,}").unwrap());

const TERMINAL: &[char] = &['.', ';', ':', '!', '?', '…'];
const KEEP_HYPHEN: &[&str] = &[
    "ci",
    "sous",
    "vice",
    "ex",
    "non",
    "demi",
    "semi",
    "mi",
    "électro",
    "adjudant",
    "monitrice",
    "procès",
];

pub fn reflow_page_text(raw: &str) -> String {
    let lines: Vec<String> = raw
        .replace('\u{c}', "\n")
        .lines()
        .map(|l| unmark(l.trim()))
        .collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for line in lines.iter().filter(|l| !l.is_empty()) {
        *seen.entry(line.as_str()).or_default() += 1;
    }
    let mut widths: Vec<usize> = lines
        .iter()
        .map(|l| l.chars().count())
        .filter(|n| (30..=160).contains(n))
        .collect();
    widths.sort_unstable();
    let width = if widths.len() >= 10 {
        widths[widths.len() * 9 / 10] as f64
    } else {
        80.0
    };

    let mut first: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut last = String::new();
    let mut blank = false;
    let mut dash: Option<&str> = None;
    for line in &lines {
        let line = line.as_str();
        if line.is_empty() {
            blank = true;
            continue;
        }
        if PAGE.is_match(line) || ARTIFACT.is_match(line) {
            continue;
        }
        if seen[line] >= 3 && running_header(line) && !first.insert(line) {
            continue;
        }
        if matches!(line, "-" | "\u{2013}" | "\u{2014}") {
            dash = Some(line);
            continue;
        }
        let carried;
        let line = match dash.take() {
            Some(d) if !MARKER.is_match(line) => {
                carried = format!("{d} {line}");
                carried.as_str()
            }
            _ => line,
        };
        if RULE.is_match(line) {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            last.clear();
            blank = false;
            continue;
        }
        if buf.is_empty() {
            buf.push_str(line);
        } else if joins(&buf, &last, line, blank, width) {
            let lower = line.starts_with(char::is_lowercase);
            match CUT_WORD.captures(&buf).map(|c| c[1].to_lowercase()) {
                Some(head) if lower && !KEEP_HYPHEN.contains(&head.as_str()) => {
                    buf.pop();
                }
                Some(_) => {}
                None => buf.push(' '),
            }
            buf.push_str(line);
        } else {
            out.push(std::mem::replace(&mut buf, line.to_string()));
        }
        line.clone_into(&mut last);
        blank = false;
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out.join("\n")
}

fn joins(buf: &str, last: &str, line: &str, blank: bool, width: f64) -> bool {
    let open = OPEN.is_match(buf);
    let long = last.chars().count() as f64 >= width * 0.6 || open;
    let terminal = buf.ends_with(TERMINAL) && !ABBREVIATION.is_match(buf);
    if buf == "***" {
        false
    } else if BULLET.is_match(line) {
        !terminal && open && !blank
    } else if MARKER.is_match(line) {
        !terminal && long && !blank
    } else if line.starts_with(char::is_lowercase) {
        !terminal || long && !buf.ends_with([';', ':'])
    } else {
        !terminal
            && (open
                || long && !heading(line)
                || heading(last) && heading(line) && LINK.is_match(line))
    }
}

/// A docket number or a court name a page repeats in its header, kept where it first appears.
fn running_header(line: &str) -> bool {
    line.chars().count() <= 40
        && line.contains(|c: char| c.is_ascii_digit())
        && !line.contains('[')
        && !LOWER_WORD.is_match(line)
}

fn heading(line: &str) -> bool {
    let letters = line.chars().filter(|c| c.is_alphabetic());
    let (n, upper) = letters.fold((0, 0), |(n, u), c| {
        (n + 1, u + usize::from(c.is_uppercase()))
    });
    n >= 4 && upper * 5 >= n * 4
}

/// The markdown an OCR leaves on a line: an image, a link, bold, italics, a table row, a heading.
fn unmark(line: &str) -> String {
    if line.len() >= 3 && line.chars().all(|c| c == '*') {
        return "***".to_string();
    }
    if RULE.is_match(line) {
        return line.to_string();
    }
    let line = MD_IMAGE.replace_all(line, "");
    let line = MD_LINK.replace_all(&line, "$1").replace("**", "");
    let mut plain = String::with_capacity(line.len());
    let mut at = 0;
    for c in ITALIC.captures_iter(&line) {
        let whole = c.get(0).unwrap();
        let before = line[..whole.start()].chars().next_back();
        let after = line[whole.end()..].chars().next();
        if [before, after]
            .into_iter()
            .flatten()
            .any(|ch| ch == '*' || ch == '_' || ch.is_alphanumeric())
        {
            continue;
        }
        plain.push_str(&line[at..whole.start()]);
        plain.push_str(&c[1]);
        at = whole.end();
    }
    plain.push_str(&line[at..]);
    let plain = if plain.starts_with('|') && plain.ends_with('|') {
        plain
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        plain
    };
    MD_HEADING.replace(&plain, "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_sentence_is_one_paragraph_and_a_cut_word_is_glued_back() {
        let raw = "Par une requête enregistrée le 4 juillet 2024, M. A demande au tribunal\n\
                   d'annuler la déci-\n\
                   sion du préfet ;\n\
                   Vu les autres pièces du dossier.";
        assert_eq!(
            reflow_page_text(raw),
            "Par une requête enregistrée le 4 juillet 2024, M. A demande au tribunal d'annuler la décision du préfet ;\n\
             Vu les autres pièces du dossier."
        );
    }

    #[test]
    fn page_numbers_and_a_repeated_docket_header_go() {
        let raw = "N° 2401234\nLe tribunal,\n- 2 -\nN° 2401234\nconsidère que\n3/7\nN° 2401234\nla requête est rejetée.";
        assert_eq!(
            reflow_page_text(raw),
            "N° 2401234\nLe tribunal, considère que la requête est rejetée."
        );
    }

    #[test]
    fn a_heading_in_capitals_stays_on_its_line_and_its_name_is_not_cut() {
        let raw = "GROUPE D'INFORMATION ET DE SOUTIEN\nDES IMMIGRÉS\nCOUR D'APPEL DE PARIS";
        assert_eq!(
            reflow_page_text(raw),
            "GROUPE D'INFORMATION ET DE SOUTIEN DES IMMIGRÉS\nCOUR D'APPEL DE PARIS"
        );
    }

    #[test]
    fn a_civility_at_the_line_end_does_not_end_the_sentence() {
        let raw = "Après avoir entendu au cours de l'audience publique les observations de M.\nDupont, avocat.";
        assert_eq!(
            reflow_page_text(raw),
            "Après avoir entendu au cours de l'audience publique les observations de M. Dupont, avocat."
        );
    }

    #[test]
    fn a_dash_alone_on_its_line_opens_the_next_one() {
        let raw = "Ont été entendus au cours de l'audience publique, à huis clos :\n-\n\nle rapport de Mme A, rapporteure ;\n-\n- les observations de Me B.";
        assert_eq!(
            reflow_page_text(raw),
            "Ont été entendus au cours de l'audience publique, à huis clos :\n- le rapport de Mme A, rapporteure ;\n- les observations de Me B."
        );
    }

    #[test]
    fn an_enumeration_keeps_one_item_per_line() {
        let raw = "Il soutient que :\n- la décision est insuffisamment motivée ;\n- elle méconnaît l'article 8.";
        assert_eq!(reflow_page_text(raw), raw);
    }

    #[test]
    fn the_markdown_an_ocr_leaves_is_dropped() {
        let raw = "# **ARRÊT**\n![img-0.jpeg](img-0.jpeg)\n| Président | M. Martin |\n|---|---|\n*Par ces motifs*, [voir](https://example.org)\n***";
        assert_eq!(
            reflow_page_text(raw),
            "ARRÊT\nPrésident M. Martin\nPar ces motifs, voir\n***"
        );
    }
}
