//! The courts' own summaries of a decision, as their registries serve them.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<(/?)([A-Za-z][A-Za-z0-9]*)((?:[^>"]|"[^"]*")*?)(/?)>|([^<]+)"#)
        .expect("regex tokens html")
});
static CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"class="([^"]*)""#).expect("regex class"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex espaces"));
static COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->|<\?.*?\?>|<!DOCTYPE[^>]*>").expect("regex"));

/// The paragraphs that head a summary or index it: the case, the parties, the rubric
/// chains, the articles and the points they refer to.
const TITRAGE: [&str; 9] = [
    "sum-title-1",
    "sum-title-2",
    "sum-title-3",
    "prelim",
    "index",
    "internal",
    "internal-right",
    "ref-abstract",
    "subjects",
];

/// Below this, a paragraph outside the titrage refers to the decision rather than
/// sums it up: « Voir le texte de la décision. ».
const PROSE_MIN_CHARS: usize = 80;

#[derive(Default)]
struct Paragraph {
    text: String,
    linked: bool,
    unlinked: bool,
    titrage: bool,
}

/// The court's summary from its CELLAR xhtml, one paragraph per line, list items numbered
/// as the court numbers them. `None` when it carries its titrage alone.
pub fn cjue_court_summary(html: &str) -> Option<String> {
    let body = match html.find("<body") {
        Some(start) => &html[start..html.rfind("</body>").unwrap_or(html.len())],
        None => html,
    };
    let body = COMMENT.replace_all(body, "");
    let mut lines: Vec<(String, bool)> = Vec::new();
    let mut paragraph = Paragraph::default();
    let mut counters: Vec<usize> = Vec::new();
    let mut items: Vec<bool> = Vec::new();
    let mut prefix: Option<String> = None;
    let mut links = 0usize;
    let mut skipped = 0usize;

    let mut flush = |paragraph: &mut Paragraph, prefix: &mut Option<String>| {
        let p = std::mem::take(paragraph);
        let text = SPACES
            .replace_all(&html_escape::decode_html_entities(&p.text), " ")
            .trim()
            .to_string();
        if text.is_empty() || (p.linked && !p.unlinked) {
            return;
        }
        let text = match prefix.take() {
            Some(prefix) => format!("{prefix} {text}"),
            None => text,
        };
        let prose = !p.titrage && text.chars().count() >= PROSE_MIN_CHARS;
        lines.push((text, prose));
    };

    for token in TOKEN.captures_iter(&body) {
        if let Some(text) = token.get(5) {
            if skipped == 0 && !text.as_str().trim().is_empty() {
                paragraph.text.push_str(text.as_str());
                if links > 0 {
                    paragraph.linked = true;
                } else {
                    paragraph.unlinked = true;
                }
            }
            continue;
        }
        let closing = &token[1] == "/";
        let tag = token[2].to_ascii_lowercase();
        if &token[4] == "/" && tag != "br" {
            continue;
        }
        let class = CLASS
            .captures(&token[3])
            .map(|c| c[1].trim_start_matches("coj-").to_string())
            .unwrap_or_default();
        match (tag.as_str(), closing) {
            ("script" | "style" | "head", false) => skipped += 1,
            ("script" | "style" | "head", true) => skipped = skipped.saturating_sub(1),
            ("a", false) => links += 1,
            ("a", true) => links = links.saturating_sub(1),
            ("br", _) => paragraph.text.push(' '),
            ("ol" | "ul", false) => {
                flush(&mut paragraph, &mut prefix);
                counters.push(0);
            }
            ("ol" | "ul", true) => {
                flush(&mut paragraph, &mut prefix);
                counters.pop();
            }
            ("li", false) => {
                flush(&mut paragraph, &mut prefix);
                if let Some(n) = counters.last_mut() {
                    *n += 1;
                    prefix = Some(format!("{n}."));
                }
                items.push(class == "subjects");
            }
            ("li", true) => {
                flush(&mut paragraph, &mut prefix);
                items.pop();
            }
            ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "td" | "tr", false) => {
                flush(&mut paragraph, &mut prefix);
                paragraph.titrage =
                    TITRAGE.contains(&class.as_str()) || items.last() == Some(&true);
            }
            ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) if class.is_empty() => {
                if paragraph_is_count(&paragraph) {
                    let count = std::mem::take(&mut paragraph);
                    prefix = Some(count.text.trim().to_string());
                } else {
                    flush(&mut paragraph, &mut prefix);
                }
            }
            _ => {}
        }
    }
    flush(&mut paragraph, &mut prefix);

    lines.iter().any(|(_, prose)| *prose).then(|| {
        lines
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// The number cell of a numbered row, `1)` or `2.`, which opens the paragraph after it.
fn paragraph_is_count(paragraph: &Paragraph) -> bool {
    let text = paragraph.text.trim();
    !text.is_empty()
        && text.len() <= 5
        && text
            .trim_end_matches([')', '.'])
            .chars()
            .all(|c| c.is_ascii_digit())
}

/// What opens a legal summary, before the month of its issue: the Information Note it
/// appeared in until 2021, the legal summary since.
const CEDH_MASTHEADS: [&str; 5] = [
    "Information Note on the Court",
    "Note d’information sur la jurisprudence",
    "Note d'information sur la jurisprudence",
    "Legal summary",
    "Résumé juridique",
];

/// The Registry's disclaimer, the last line kept: the links to the collection follow it.
const CEDH_DISCLAIMERS: [&str; 2] = ["does not bind the Court", "ne lie pas la Cour"];

/// The line that stands in for the summary until the Registry has translated it.
const CEDH_PENDING: [&str; 2] = [
    "TRADUCTION DU RÉSUMÉ JURIDIQUE EN COURS",
    "LEGAL SUMMARY PENDING TRANSLATION",
];

/// Whether a HUDOC legal summary holds its heading alone, its translation pending.
pub fn cedh_legal_summary_pending(text: &str) -> bool {
    text.lines().any(|l| CEDH_PENDING.contains(&l.trim()))
}

/// The Registry's legal summary from its HUDOC text, from the case to the disclaimer.
/// `None` on any other layout, and while its translation is pending.
pub fn cedh_legal_summary(text: &str) -> Option<String> {
    if cedh_legal_summary_pending(text) {
        return None;
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let masthead = lines.first()?;
    if !CEDH_MASTHEADS.iter().any(|m| masthead.starts_with(m)) {
        return None;
    }
    let end = lines
        .iter()
        .rposition(|l| CEDH_DISCLAIMERS.iter().any(|d| l.contains(d)))?;
    (end > 2).then(|| lines[2..=end].join("\n"))
}

static ARRET_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[ \u{a0}]*\(\s*((?:et\s+)?arr[êe]ts?\b[^()\n]*)\)").expect("regex marker")
});
static ARRET_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)arr[êe]ts?").expect("regex arrêt"));
static ARRET_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^arr[êe]ts?\s*(?:n°\s*s?|nos?|n\.\s*s?|numéros?|n)?\s*",
        r"(\d+(?:\s*(?:,|et|à)\s*(?:n°\s*)?\d+)*)",
        r"(?:\s*,\s*pourvois?\s*(?:n°\s*s?|nos?|n\.)?\s*",
        r"(\d{2}-\d{2}\.\d{3}(?:\s*(?:,|et)\s*(?:n°\s*)?\d{2}-\d{2}\.\d{3})*))?",
        r"[\s,;]*(?:et)?\s*$",
    ))
    .expect("regex marker item")
});
static ARRET_NUMBERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)(?:\s*à\s*(\d+))?").expect("regex numbers"));
static DOCKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{2}-\d{2}\.\d{3}").expect("regex docket"));
static ARRET_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*arr[êe]t\s+n\s*[°o]\s*(\d+)\s*\n").expect("regex head"));

/// A « (arrêts n°s 1 et 2) » or « (arrêt n° 1, pourvoi n° 07-10.055) » of a summary
/// shared by the arrêts the Bulletin published together.
struct ArretMarker {
    span: Range<usize>,
    items: Vec<(Vec<u32>, Vec<String>)>,
}

impl ArretMarker {
    fn parse(span: Range<usize>, content: &str) -> Option<Self> {
        let starts: Vec<usize> = ARRET_WORD.find_iter(content).map(|m| m.start()).collect();
        if !matches!(content[..*starts.first()?].trim(), "" | "et") {
            return None;
        }
        let ends = starts.iter().skip(1).copied().chain([content.len()]);
        let items = starts
            .iter()
            .zip(ends)
            .map(|(&a, b)| {
                let item = ARRET_ITEM.captures(&content[a..b])?;
                let arrets = ARRET_NUMBERS
                    .captures_iter(&item[1])
                    .flat_map(|n| {
                        let first: u32 = n[1].parse().unwrap_or(0);
                        let last = n.get(2).map_or(first, |l| l.as_str().parse().unwrap_or(0));
                        first..=last
                    })
                    .collect();
                let pourvois = item.get(2).map_or_else(Vec::new, |p| {
                    DOCKET
                        .find_iter(p.as_str())
                        .map(|d| d.as_str().to_string())
                        .collect()
                });
                Some((arrets, pourvois))
            })
            .collect::<Option<_>>()?;
        Some(Self { span, items })
    }

    fn arrets(&self) -> BTreeSet<u32> {
        self.items
            .iter()
            .flat_map(|(a, _)| a.iter().copied())
            .collect()
    }
}

fn arret_markers(summary: &str) -> Vec<ArretMarker> {
    ARRET_MARKER
        .captures_iter(summary)
        .filter_map(|c| ArretMarker::parse(c.get(0)?.range(), c.get(1)?.as_str()))
        .collect()
}

/// Whether a Cour de cassation summary names its arrêts by their number in the Bulletin.
pub fn cassation_summary_numbers_arrets(summary: &str) -> bool {
    !arret_markers(summary).is_empty()
}

/// The Cour de cassation's summary as it reads on one of the arrêts published under it,
/// `twins` the others' text heads and pourvois. Each number becomes its pourvoi once all are
/// known; else the arrêt's own markers are cut, untouched when it is not known and named.
pub fn cassation_summary_for(
    summary: &str,
    text: &str,
    dockets: &[String],
    twins: &[(String, Vec<String>)],
) -> String {
    let markers = arret_markers(summary);
    if markers.is_empty() {
        return summary.to_string();
    }
    if let Some(named) = named_by_pourvoi(summary, &markers, text, dockets, twins) {
        return named;
    }
    let own = markers
        .iter()
        .flat_map(|m| &m.items)
        .find_map(|(arrets, pourvois)| {
            (arrets.len() == 1 && pourvois.iter().any(|p| dockets.contains(p))).then(|| arrets[0])
        })
        .or_else(|| ARRET_HEAD.captures(text).and_then(|c| c[1].parse().ok()));
    let own = match own {
        Some(n) if markers.iter().any(|m| m.arrets().contains(&n)) => Some(n),
        None if markers[0].arrets().len() >= 2
            && markers.iter().all(|m| m.arrets() == markers[0].arrets()) =>
        {
            None
        }
        _ => return summary.to_string(),
    };
    let mut out: Vec<String> = Vec::new();
    let mut start = 0;
    for line in summary.split('\n') {
        let end = start + line.len();
        let here: Vec<&ArretMarker> = markers
            .iter()
            .filter(|m| (start..=end).contains(&m.span.start))
            .collect();
        let cuts: Vec<&Range<usize>> = here
            .iter()
            .filter(|m| own.is_none_or(|n| m.arrets().contains(&n)))
            .map(|m| &m.span)
            .collect();
        if here.is_empty() || !cuts.is_empty() {
            let mut kept = String::with_capacity(line.len());
            let mut from = start;
            for cut in cuts {
                kept.push_str(&summary[from..cut.start]);
                from = cut.end;
            }
            kept.push_str(&summary[from..end]);
            out.push(kept);
        }
        start = end + 1;
    }
    out.join("\n")
}

/// The summary with each marker naming its arrêts by pourvoi, `None` while a number it
/// names has no pourvoi or two.
fn named_by_pourvoi(
    summary: &str,
    markers: &[ArretMarker],
    text: &str,
    dockets: &[String],
    twins: &[(String, Vec<String>)],
) -> Option<String> {
    let headed = std::iter::once((text, dockets))
        .chain(twins.iter().map(|(t, d)| (t.as_str(), d.as_slice())))
        .filter_map(|(t, d)| {
            let n = ARRET_HEAD.captures(t)?[1].parse().ok()?;
            Some((n, d.first()?.as_str()))
        });
    let stated =
        markers
            .iter()
            .flat_map(|m| &m.items)
            .filter_map(|(arrets, pourvois)| match arrets.as_slice() {
                [n] => Some((*n, pourvois.first()?.as_str())),
                _ => None,
            });
    let mut pourvoi_of: BTreeMap<u32, Option<&str>> = BTreeMap::new();
    for (n, pourvoi) in headed.chain(stated) {
        pourvoi_of
            .entry(n)
            .and_modify(|known| {
                if *known != Some(pourvoi) {
                    *known = None;
                }
            })
            .or_insert(Some(pourvoi));
    }
    let mut out = String::with_capacity(summary.len());
    let mut from = 0;
    for marker in markers {
        let named: Vec<String> = marker
            .items
            .iter()
            .flat_map(|(arrets, _)| arrets)
            .map(|n| Some(format!("n° {}", pourvoi_of.get(n).copied().flatten()?)))
            .collect::<Option<_>>()?;
        let (last, rest) = named.split_last()?;
        let open = marker.span.start + summary[marker.span.clone()].find('(')?;
        out.push_str(&summary[from..open]);
        out.push_str(&if rest.is_empty() {
            format!("(arrêt {last})")
        } else {
            format!("(arrêts {} et {last})", rest.join(", "))
        });
        from = marker.span.end;
    }
    out.push_str(&summary[from..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARED: &str =
        "Le médecin est tenu d'une obligation de sécurité de résultat (arrêts nos 1 et 2).";
    const POINTS: &str = "Règle commune.\n Premier point (arrêt n° 1).\n Second point (Arrêt n. 2), peu important la suite.";

    #[test]
    fn a_shared_cassation_summary_names_its_arrets_by_pourvoi() {
        let dockets = ["97-21.903".to_string()];
        let twin = [(
            "ARRÊT N° 1\n\nAttendu".to_string(),
            vec!["97-15.818".to_string()],
        )];
        assert_eq!(
            cassation_summary_for(SHARED, "ARRÊT N° 2\n\nSur le moyen", &dockets, &twin),
            "Le médecin est tenu d'une obligation de sécurité de résultat (arrêts n° 97-15.818 et n° 97-21.903)."
        );
        assert_eq!(
            cassation_summary_for(POINTS, "ARRÊT N° 2\n\nSur le moyen", &dockets, &twin),
            "Règle commune.\n Premier point (arrêt n° 97-15.818).\n Second point (arrêt n° 97-21.903), peu important la suite."
        );
        let stated = "Point A (arrêt n° 1, pourvoi n° 07-10.055) ou point B (arrêt n° 2, pourvoi n° 97-21.903).";
        assert_eq!(
            cassation_summary_for(stated, "", &dockets, &[]),
            "Point A (arrêt n° 07-10.055) ou point B (arrêt n° 97-21.903)."
        );
    }

    #[test]
    fn without_its_twins_pourvois_a_shared_cassation_summary_reads_as_the_arrets_own() {
        let dockets = ["97-21.903".to_string()];
        assert_eq!(
            cassation_summary_for(SHARED, "Sur le moyen unique :", &dockets, &[]),
            "Le médecin est tenu d'une obligation de sécurité de résultat."
        );
        assert_eq!(
            cassation_summary_for(POINTS, "ARRÊT N° 2\n\nSur le moyen", &dockets, &[]),
            "Règle commune.\n Second point, peu important la suite."
        );
        assert_eq!(
            cassation_summary_for(POINTS, "Arrêt n° 275 FS-P+B\nSur le moyen", &dockets, &[]),
            POINTS
        );
        assert_eq!(
            cassation_summary_for(POINTS, "Sur le moyen unique :", &dockets, &[]),
            POINTS
        );
        let half = "Point A (arrêt n° 1) ou point B (arrêt n° 2, pourvoi n° 97-21.903).";
        assert_eq!(
            cassation_summary_for(half, "", &dockets, &[]),
            "Point A (arrêt n° 1) ou point B."
        );
        let other = "Seul point (arrêt n° 1).";
        assert_eq!(
            cassation_summary_for(other, "ARRÊT N° 2\n", &dockets, &[]),
            other
        );
        let cited = "Voir déjà (arrêt n° 84-11.111) et (CJUE, arrêt 11 juillet 2002, C-371/99).";
        assert_eq!(cassation_summary_for(cited, "", &dockets, &[]), cited);
    }

    const RULE: &str = "Les autorités nationales de contrôle disposent d’un large éventail de pouvoirs, dont celui d’examiner une demande.";

    #[test]
    fn a_summary_keeps_its_titrage_and_numbers_its_points() {
        let html = format!(
            r#"<?xml version="1.0"?><html><head><style>p {{}}</style></head><body>
<p class="sum-title-1"><span class="bold">Affaire C-362/14</span></p>
<p class="sum-title-2">Maximillian Schrems</p>
<p class="index">«Renvoi préjudiciel - Données à caractère personnel»</p>
<ol><li class="subjects"><p>Rapprochement des législations - Protection des données</p>
<p class="internal">(Art. 16, § 2, TFUE; directive 95/46, art. 28, § 1<span class="super">er</span>)</p></li></ol>
<ol><li class="normal"><p id="point1"> Voir le texte de la décision.</p><p class="ref-abstract">(cf. point 38)</p></li>
<li class="normal"><p id="point2"> {RULE}</p><p class="normal">S’agissant des transferts,<br/>la directive le prévoit.</p></li></ol>
</body></html>"#
        );
        assert_eq!(
            cjue_court_summary(&html).as_deref(),
            Some(
                format!(
                    "Affaire C-362/14\nMaximillian Schrems\n«Renvoi préjudiciel - Données à caractère personnel»\n\
1. Rapprochement des législations - Protection des données\n(Art. 16, § 2, TFUE; directive 95/46, art. 28, § 1er)\n\
1. Voir le texte de la décision.\n(cf. point 38)\n2. {RULE}\nS’agissant des transferts, la directive le prévoit."
                )
                .as_str()
            )
        );
    }

    #[test]
    fn a_summary_of_rubric_chains_alone_is_none() {
        let html = r#"<body><p class="coj-sum-title-1">Affaire C-311/19</p>
<p class="coj-index">« Renvoi préjudiciel - Libre prestation des services - Restrictions - Réglementation nationale »</p>
<ol><li class="coj-subjects"><p>Questions préjudicielles - Compétence de la Cour - Limites - Question soulevée à propos d’un litige cantonné à un seul État membre</p>
<p class="coj-internal-right">(voir points 23-25)</p></li></ol>
<p class="coj-normal"><a href="https://curia.europa.eu/juris/documents.jsf?num=C-311/19">Voir le texte de la décision</a></p></body>"#;
        assert_eq!(cjue_court_summary(html), None);
    }

    #[test]
    fn an_information_note_joins_each_number_to_its_row() {
        let html = format!(
            r#"<body><p class="coj-sum-title-1">Ordonnance du Tribunal du 6 avril 2017 -<br/>Perfect Bar/EUIPO</p>
<p class="coj-sum-title-1"><span class="coj-bold">Objet</span></p>
<p class="coj-normal">{RULE}</p>
<p class="coj-sum-title-1"><span class="coj-bold">Dispositif</span></p>
<table><col width="5%"/><tr><td><p class="coj-count">1) </p></td><td><p class="coj-normal">Le recours est rejeté.</p></td></tr></table>
<table><tr><td><p class="count">2) </p></td><td><p class="normal">Perfect Bar est condamnée aux dépens.</p></td></tr></table></body>"#
        );
        assert_eq!(
            cjue_court_summary(&html).as_deref(),
            Some(
                format!(
                    "Ordonnance du Tribunal du 6 avril 2017 - Perfect Bar/EUIPO\nObjet\n{RULE}\nDispositif\n\
1) Le recours est rejeté.\n2) Perfect Bar est condamnée aux dépens."
                )
                .as_str()
            )
        );
    }

    #[test]
    fn a_legal_summary_runs_from_the_case_to_the_disclaimer() {
        let text = "Résumé juridique\nDécembre 2024\nHasmik Khachatryan c. Arménie - 11829/16\n\
Arrêt 12.12.2024 [Section V]\nArticle 3\nObligations positives\nConclusion : violation (unanimité).\n\
© Conseil de l’Europe/Cour européenne des droits de l’homme\nRédigé par le greffe, ce résumé ne lie pas la Cour.\n\
Pour accéder aux résumés juridiques en français ou en anglais cliquez ici.";
        assert_eq!(
            cedh_legal_summary(text).as_deref(),
            Some(
                "Hasmik Khachatryan c. Arménie - 11829/16\nArrêt 12.12.2024 [Section V]\nArticle 3\n\
Obligations positives\nConclusion : violation (unanimité).\n\
© Conseil de l’Europe/Cour européenne des droits de l’homme\nRédigé par le greffe, ce résumé ne lie pas la Cour."
            )
        );
        let old = "Information Note on the Court’s case-law No. 71\nJanuary 2005\nRamirez Sanchez v. France - 59450/00\n\
Judgment 27.1.2005 [Section I]\nThis summary by the Registry does not bind the Court.\nClick here for the Case-Law Information Notes";
        assert_eq!(
            cedh_legal_summary(old).as_deref(),
            Some("Ramirez Sanchez v. France - 59450/00\nJudgment 27.1.2005 [Section I]\nThis summary by the Registry does not bind the Court.")
        );
        assert_eq!(cedh_legal_summary("AFFAIRE X c. FRANCE\nArrêt"), None);
        let pending = "Résumé juridique\nOctobre 2026\nX c. Ukraine - 52652/20\nArrêt 15.10.2026 [Section V]\n\
TRADUCTION DU RÉSUMÉ JURIDIQUE EN COURS\nRédigé par le greffe, ce résumé ne lie pas la Cour.";
        assert!(cedh_legal_summary_pending(pending));
        assert_eq!(cedh_legal_summary(pending), None);
        assert!(!cedh_legal_summary_pending(text));
    }
}
