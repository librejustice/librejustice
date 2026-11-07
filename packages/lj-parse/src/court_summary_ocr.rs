//! The CJUE's summaries printed in the Reports before 2005, read back from the OCR of
//! their scanned pages.

use std::sync::LazyLock;

use regex::Regex;

/// A block of an OCR'd page: its kind (`text`, `title`, `list`, `table`, `header`,
/// `footer`, `references`…), its box and its markdown.
pub struct OcrBlock {
    pub kind: String,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub content: String,
}

pub struct OcrPage {
    pub width: f64,
    pub blocks: Vec<OcrBlock>,
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#+\s*").expect("regex"));
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!?\[[^\]]*\]\([^)]*\)").expect("regex"));
static FOOTNOTE_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*\$\^\{\s*(\d+|\\?\*)\s*\}\$|\s*[¹²³⁴⁵⁶⁷⁸⁹⁰]+").expect("regex")
});
static SUPERSCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\^\{\s*([^}]*?)\s*\}\$").expect("regex"));
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([*_#()\[\]$-])").expect("regex"));
static LEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*(\.{3,}|…)\s*").expect("regex"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));
static POINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[.)]\s").expect("regex"));
static DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\u{2014}\s*| \u{2013} | - \D").expect("regex"));
static CASE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+ ?[/-] ?\d{2}\b").expect("regex"));

/// `61978CJ0267_SUM` → `267/78`, the number the Reports give the case.
pub fn cjue_case_number(celex: &str) -> Option<String> {
    let year = celex.get(3..5)?;
    let number = celex.get(7..11)?.trim_start_matches('0');
    (year.bytes().all(|b| b.is_ascii_digit())
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit()))
    .then(|| format!("{number}/{year}"))
}

/// The summary of case `case` (`267/78`) from the OCR of its pages, laid out as [`crate::cjue_court_summary`]
/// lays out the xhtml. `None` when no rubric chain opens it, or when it carries its titrage alone.
pub fn cjue_court_summary_ocr(pages: &[OcrPage], case: &str) -> Option<String> {
    let (number, year) = case.split_once('/')?;
    // Joined cases name the first one before the year they share: `4 to 13/59`, `43, 45 and 48/59`.
    let case = Regex::new(&format!(
        r"(^|[^0-9]){number}(\D{{1,12}}\d+){{0,8}} ?[/-] ?{year}([^0-9]|$)"
    ))
    .expect("regex case");
    if !pages
        .first()
        .is_some_and(|page| page.blocks.iter().any(|b| case.is_match(&b.content)))
    {
        return None;
    }
    let mut blocks: Vec<(bool, Vec<String>)> = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        for block in reading_order(page) {
            let lines: Vec<String> = match block.kind.as_str() {
                "text" | "title" | "list" => block
                    .content
                    .replace("* (", "*\n(")
                    .lines()
                    .map(clean)
                    .collect(),
                "header" if i == 0 => vec![clean(&block.content)],
                "footer" if rubric_chain(&clean(&block.content)) => {
                    block.content.lines().map(clean).collect()
                }
                "table" => block.content.lines().filter_map(table_row).collect(),
                _ => continue,
            };
            let lines: Vec<String> = lines.into_iter().filter(|l| !l.is_empty()).collect();
            if !lines.is_empty() {
                blocks.push((block.kind == "title", lines));
            }
        }
    }
    // A block over the case number in brackets opens a decision of a page of unpublished ones:
    // no rubric, and what is above it ends the decision before.
    let heading = |n: usize| {
        blocks.get(n + 1).is_some_and(|(_, next)| {
            next.len() == 1
                && next[0].starts_with('(')
                && next[0].chars().count() < 60
                && case.is_match(&next[0])
        })
    };
    let start = (0..blocks.len()).find(|&n| {
        !heading(n)
            && match blocks[n].1.split_last() {
                Some((last, rest)) if !rest.is_empty() && reference(last) => {
                    rubric_dashes(&rest.join(" ")) >= 1
                }
                _ => rubric_chain(&blocks[n].1.join(" ")),
            }
    })?;
    let first = (0..start).rev().find(|&n| heading(n)).unwrap_or(0);
    // The summary runs to the report or the decision that opens on its case, or to the
    // next decision of a page of unpublished ones, a title over another case number.
    let ends = |n: usize| {
        let (title, lines) = &blocks[n];
        let text = lines.join(" ");
        let named = |text: &str| case.is_match(text);
        if *title {
            !POINT.is_match(&text)
                && !rubric_chain(&text)
                && (named(&text)
                    || !text.chars().any(char::is_lowercase)
                    || blocks.get(n + 1).is_some_and(|(_, next)| {
                        next.len() == 1
                            && next[0].starts_with('(')
                            && next[0].chars().count() < 60
                            && CASE.is_match(&next[0])
                            && !named(&next[0])
                    }))
        } else {
            lines.len() == 1 && text.chars().count() < 150 && named(&text)
        }
    };
    let end = (start + 1..blocks.len())
        .find(|&n| ends(n))
        .unwrap_or(blocks.len());

    let mut head: Vec<String> = Vec::new();
    for (_, lines) in &blocks[first..start] {
        for (n, line) in lines.iter().enumerate() {
            match head.last_mut() {
                Some(last)
                    if open(last) || (n > 0 && !last.ends_with(|c: char| c.is_ascii_digit())) =>
                {
                    join(last, line)
                }
                _ => head.push(line.clone()),
            }
        }
    }
    let mut body: Vec<String> = Vec::new();
    let mut after_title = false;
    for (title, lines) in &blocks[start..end] {
        for (n, line) in lines.iter().enumerate() {
            let fresh = line.starts_with('(')
                || POINT.is_match(line)
                || (n == 0 && (*title || after_title));
            match body.last_mut() {
                Some(last) if !fresh && !reference(last) && (n > 0 || crosses(last, line)) => {
                    join(last, line)
                }
                _ => body.push(line.clone()),
            }
        }
        after_title = *title;
    }
    body.iter()
        .any(|line| statement(line))
        .then(|| head.into_iter().chain(body).collect::<Vec<_>>().join("\n"))
}

/// A page's blocks in reading order: a block across the gutter closes what is above it,
/// and the strips of two columns between two such blocks read left column first.
fn reading_order(page: &OcrPage) -> Vec<&OcrBlock> {
    let mid = page.width / 2.0;
    let reach = page.width / 10.0;
    let mut blocks: Vec<&OcrBlock> = page.blocks.iter().collect();
    blocks.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut out = Vec::new();
    let mut region = Vec::new();
    for block in blocks {
        if block.x0 < mid - reach && block.x1 > mid + reach {
            columns(&mut region, mid, &mut out);
            out.push(block);
        } else {
            region.push(block);
        }
    }
    columns(&mut region, mid, &mut out);
    out
}

/// Strips are runs of blocks overlapping in height: those with a block on each side make
/// one pair of columns, read left then right, and a strip on one side alone reads as is.
fn columns<'a>(region: &mut Vec<&'a OcrBlock>, mid: f64, out: &mut Vec<&'a OcrBlock>) {
    let left = |b: &&OcrBlock| (b.x0 + b.x1) / 2.0 < mid;
    let mut strips: Vec<Vec<&OcrBlock>> = Vec::new();
    let mut bottom = f64::NEG_INFINITY;
    for block in region.drain(..) {
        if block.y0 > bottom {
            strips.push(Vec::new());
        }
        bottom = bottom.max(block.y1);
        strips.last_mut().expect("a strip").push(block);
    }
    let flush = |pair: &mut Vec<&'a OcrBlock>, out: &mut Vec<&'a OcrBlock>| {
        let (l, r): (Vec<_>, Vec<_>) = pair.drain(..).partition(left);
        out.extend(l);
        out.extend(r);
    };
    let mut pair: Vec<&OcrBlock> = Vec::new();
    for strip in strips {
        if strip.iter().any(left) && !strip.iter().all(left) {
            pair.extend(strip);
        } else {
            flush(&mut pair, out);
            out.extend(strip);
        }
    }
    flush(&mut pair, out);
}

fn clean(line: &str) -> String {
    let line = HEADING.replace(line.trim(), "");
    let line = LINK.replace_all(&line, "");
    let line = FOOTNOTE_MARK.replace_all(&line, "");
    let line = SUPERSCRIPT.replace_all(&line, "$1");
    let line = line.replace("**", "").replace("\\*", "").replace('*', "");
    let line = ESCAPE.replace_all(&line, "$1");
    let line = LEADER.replace_all(&line, " ");
    let line = line.replace("[Non-Text]", "");
    let line = line.trim();
    let line = line.strip_prefix("- ").unwrap_or(line);
    if !line.chars().any(char::is_alphanumeric) {
        return String::new();
    }
    SPACES.replace_all(line, " ").trim().to_string()
}

/// A markdown table row as its cells, the separator row as nothing.
fn table_row(row: &str) -> Option<String> {
    let cells: Vec<&str> = row
        .trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();
    (!cells
        .iter()
        .all(|c| c.chars().all(|ch| ch == '-' || ch == ':')))
    .then(|| clean(&cells.join(" ")))
}

/// Two dashes at least, outside the subject the head sets in quotes or brackets.
fn rubric_chain(line: &str) -> bool {
    rubric_dashes(line) >= 2
}

/// The dashes of a line that opens on a rubric's subject, a dash within its first 110
/// characters: past a rubric's subject, short of a statement's first dash.
fn rubric_dashes(line: &str) -> usize {
    let line = POINT.replace(line, "");
    let dashes: Vec<usize> = DASH.find_iter(&line).map(|m| m.start()).collect();
    match dashes.first() {
        Some(&first)
            if !line.starts_with(['«', '„', '“', '"', '(', '‹', '['])
                && line[..first].chars().count() <= 110 =>
        {
            dashes.len()
        }
        _ => 0,
    }
}

/// A line of the court's own words, a rubric's shape aside unless it closes as a sentence:
/// a rubric closed by a period runs shorter segments than a statement.
fn statement(line: &str) -> bool {
    let len = line.chars().count();
    len >= 80
        && !reference(line)
        && (!rubric_chain(line)
            || (line.ends_with('.') && len > 75 * (DASH.find_iter(line).count() + 1)))
}

/// A reference closes its line: `(Traité CE, art. 119)`, `(see paras 53, 55-58)`.
fn reference(line: &str) -> bool {
    line.starts_with('(') && line.ends_with(')')
}

/// A line cut mid-sentence by a column or a page.
fn continues(line: &str) -> bool {
    line.ends_with(|c: char| {
        c.is_alphanumeric() || matches!(c, ',' | '\u{2014}' | '\u{2013}' | '-')
    })
}

/// A line a new block carries on: a rubric that does not end on a dash ends there, its
/// statement opening on a capital.
fn crosses(last: &str, next: &str) -> bool {
    continues(last)
        && (!rubric_chain(last)
            || last.ends_with(['\u{2014}', '\u{2013}'])
            || next.starts_with(char::is_lowercase))
}

/// A head line left open: a bracket or a quote, a comma or a dash at its end.
fn open(line: &str) -> bool {
    let count = |c: char| line.matches(c).count();
    count('(') > count(')')
        || count('«') > count('»')
        || (count('„') > 0 && count('„') > count('“') + count('”'))
        || (count('„') == 0 && count('“') > count('”'))
        || line.ends_with([',', '\u{2014}', '\u{2013}', '-'])
}

/// Appends `next` to `line`, the word a hyphen cut at a line's end put back together.
fn join(line: &mut String, next: &str) {
    let hyphen = line.ends_with('-')
        && line.chars().rev().nth(1).is_some_and(char::is_alphabetic)
        && next.starts_with(char::is_lowercase);
    if hyphen {
        line.pop();
    } else {
        line.push(' ');
    }
    line.push_str(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: &str, x0: f64, y0: f64, x1: f64, y1: f64, content: &str) -> OcrBlock {
        OcrBlock {
            kind: kind.into(),
            x0,
            y0,
            x1,
            y1,
            content: content.into(),
        }
    }

    #[test]
    fn the_case_number_is_read_from_the_celex() {
        assert_eq!(
            cjue_case_number("61978CJ0267_SUM").as_deref(),
            Some("267/78")
        );
        assert_eq!(
            cjue_case_number("62004TO0094_SUM").as_deref(),
            Some("94/04")
        );
    }

    #[test]
    fn a_summary_reads_its_columns_in_order_and_stops_at_its_decision() {
        let pages = [
            OcrPage {
                width: 650.0,
                blocks: vec![
                    block("title", 203.0, 78.0, 446.0, 110.0, "# JUDGMENT OF THE COURT\nOF 10 JANUARY 1980$^{1}$"),
                    block("title", 141.0, 178.0, 510.0, 220.0, "# **Commission of the European Communities**\n**v Italian Republic**"),
                    block("text", 46.0, 241.0, 603.0, 290.0, "“Failure of a State to fulfil its obligations \u{2014} Own resources \u{2014}\nPrivilege of non-disclosure”"),
                    block("text", 287.0, 360.0, 362.0, 380.0, "Case 267/78"),
                    block("text", 48.0, 438.0, 605.0, 480.0, "1. *European Communities \u{2014} Own resources \u{2014} Verifications \u{2014}\nPowers of the Commission*\n(EEC Treaty, Art. 5)"),
                    block("text", 48.0, 621.0, 321.0, 890.0, "1. A Member State may not contest the Commission’s power to exercise its super-\nvision as soon as the resources have been established, nor"),
                    block("text", 352.0, 621.0, 605.0, 640.0, "when the resources ought to have been established."),
                    block("text", 334.0, 653.0, 607.0, 890.0, "2. Rules of criminal law may be relied upon against the Commission in so far as they may be relied upon against the national authorities."),
                    block("footer", 53.0, 909.0, 214.0, 920.0, "$^{1}$ \u{2014} Language of the Case: Italian."),
                    block("footer", 586.0, 939.0, 607.0, 950.0, "31"),
                ],
            },
            OcrPage {
                width: 650.0,
                blocks: vec![
                    block("header", 242.0, 71.0, 464.0, 85.0, "JUDGMENT OF 10. 1. 1980 \u{2014} CASE 267/78"),
                    block("text", 78.0, 237.0, 192.0, 250.0, "In Case 267/78"),
                    block("text", 78.0, 281.0, 630.0, 380.0, "COMMISSION OF THE EUROPEAN COMMUNITIES, represented by its Legal Adviser,"),
                ],
            },
        ];
        assert_eq!(
            cjue_court_summary_ocr(&pages, "267/78").as_deref(),
            Some(
                "JUDGMENT OF THE COURT OF 10 JANUARY 1980\nCommission of the European Communities v Italian Republic\n\
“Failure of a State to fulfil its obligations \u{2014} Own resources \u{2014} Privilege of non-disclosure”\nCase 267/78\n\
1. European Communities \u{2014} Own resources \u{2014} Verifications \u{2014} Powers of the Commission\n(EEC Treaty, Art. 5)\n\
1. A Member State may not contest the Commission’s power to exercise its supervision as soon as the resources have been established, nor when the resources ought to have been established.\n\
2. Rules of criminal law may be relied upon against the Commission in so far as they may be relied upon against the national authorities."
            )
        );
    }

    #[test]
    fn an_unpublished_decision_runs_to_the_next_one() {
        let pages = [OcrPage {
            width: 720.0,
            blocks: vec![
                block("title", 140.0, 60.0, 580.0, 90.0, "# **Ordonnance de la Cour du 16 juin 2010 \u{2014} RANI / Hankook**"),
                block("text", 294.0, 100.0, 427.0, 115.0, "(affaire C-298/09)"),
                block("list", 69.0, 130.0, 653.0, 170.0, "1. *Libre prestation des services \u{2014} Traité \u{2014} Applicabilité*"),
                block("title", 69.0, 180.0, 115.0, 190.0, "# **Objet**"),
                block("text", 69.0, 200.0, 653.0, 230.0, "Demande de décision préjudicielle \u{2014} Fővárosi Bíróság \u{2014} Interprétation de l'art. 3 CE"),
                block("title", 69.0, 240.0, 147.0, 250.0, "# **Dispositif**"),
                block("list", 69.0, 260.0, 653.0, 300.0, "1) Les articles 49 CE à 54 CE ne sauraient être interprétés en ce sens qu'une telle réglementation leur est contraire."),
                block("title", 136.0, 320.0, 584.0, 340.0, "# **Ordonnance de la Cour du 17 juin 2010 \u{2014} Michalias**"),
                block("text", 294.0, 350.0, 427.0, 365.0, "**(affaire C-312/09)**"),
                block("list", 69.0, 380.0, 653.0, 420.0, "1. Procédure \u{2014} Questions préjudicielles \u{2014} Irrecevabilité"),
            ],
        }];
        assert_eq!(
            cjue_court_summary_ocr(&pages, "298/09").as_deref(),
            Some(
                "Ordonnance de la Cour du 16 juin 2010 \u{2014} RANI / Hankook\n(affaire C-298/09)\n\
1. Libre prestation des services \u{2014} Traité \u{2014} Applicabilité\nObjet\n\
Demande de décision préjudicielle \u{2014} Fővárosi Bíróság \u{2014} Interprétation de l'art. 3 CE\nDispositif\n\
1) Les articles 49 CE à 54 CE ne sauraient être interprétés en ce sens qu'une telle réglementation leur est contraire."
            )
        );
    }

    #[test]
    fn an_unnumbered_rubric_ends_where_its_statement_opens() {
        let pages = [OcrPage {
            width: 700.0,
            blocks: vec![
                block("header", 290.0, 90.0, 440.0, 105.0, "Affaire 70/86"),
                block("title", 260.0, 650.0, 440.0, 670.0, "# Sommaire de l'arrêt"),
                block("text", 38.0, 741.0, 439.0, 760.0, "*Droit communautaire \u{2014} Principes \u{2014} Force majeure \u{2014} Notion*"),
                block("text", 38.0, 812.0, 327.0, 900.0, "La notion de force majeure vise essentiellement des circonstances étrangères à celui qui l'invoque,"),
                block("text", 339.0, 812.0, 626.0, 900.0, "anormales et imprévisibles, dont les conséquences n'auraient pu être évitées."),
            ],
        }];
        assert_eq!(
            cjue_court_summary_ocr(&pages, "70/86").as_deref(),
            Some(
                "Affaire 70/86\nSommaire de l'arrêt\n\
Droit communautaire \u{2014} Principes \u{2014} Force majeure \u{2014} Notion\n\
La notion de force majeure vise essentiellement des circonstances étrangères à celui qui l'invoque, anormales et imprévisibles, dont les conséquences n'auraient pu être évitées."
            )
        );
    }

    #[test]
    fn a_statement_whose_dashes_come_early_is_no_rubric() {
        let pages = [OcrPage {
            width: 700.0,
            blocks: vec![
                block("text", 276.0, 95.0, 428.0, 110.0, "Case T-260/97 R"),
                block("text", 34.0, 723.0, 671.0, 760.0, "*Applications for interim measures \u{2014} Suspension of operation of a measure \u{2014} Conditions for granting \u{2014} Urgency*"),
                block("text", 34.0, 779.0, 456.0, 800.0, "*(Rules of Procedure of the Court of First Instance, Art. 104(2))*"),
                block("text", 34.0, 837.0, 347.0, 900.0, "It is necessary to assess urgency \u{2014} which is a necessary condition for the suspension of operation of a measure and for"),
                block("text", 361.0, 837.0, 671.0, 900.0, "other interim measures \u{2014} by examining whether, pending the Court's decision on the substance, the applicant could suffer serious and irreversible harm which could not be made good by the judgment in the main proceedings."),
            ],
        }];
        assert_eq!(
            cjue_court_summary_ocr(&pages, "260/97").as_deref(),
            Some(
                "Case T-260/97 R\n\
Applications for interim measures \u{2014} Suspension of operation of a measure \u{2014} Conditions for granting \u{2014} Urgency\n\
(Rules of Procedure of the Court of First Instance, Art. 104(2))\n\
It is necessary to assess urgency \u{2014} which is a necessary condition for the suspension of operation of a measure and for other interim measures \u{2014} by examining whether, pending the Court's decision on the substance, the applicant could suffer serious and irreversible harm which could not be made good by the judgment in the main proceedings."
            )
        );
    }

    #[test]
    fn a_summary_of_its_titrage_alone_is_none() {
        let pages = [OcrPage {
            width: 700.0,
            blocks: vec![
                block("title", 300.0, 50.0, 400.0, 60.0, "# Affaire T-1/99"),
                block("list", 60.0, 100.0, 640.0, 160.0, "1. Procédure \u{2014} Recevabilité \u{2014} Délai de recours\n2. Voir le texte de la décision."),
                block("title", 60.0, 300.0, 640.0, 320.0, "## RAPPORT D'AUDIENCE présenté dans l'affaire T-1/99\\*"),
                block("text", 60.0, 340.0, 640.0, 600.0, "Le requérant, fonctionnaire de la Commission, a introduit une réclamation au titre de l'article 90 du statut, rejetée par une décision explicite."),
            ],
        }];
        assert_eq!(cjue_court_summary_ocr(&pages, "1/99"), None);
    }
}
