//! Justel, the consolidated legislation of the Belgian Moniteur: the articles of one page, current
//! or archived, with their division path and the days the amendments they carry took effect.

use std::sync::LazyLock;

use regex::{Captures, Regex};

/// An article as one Justel page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JustelArticle {
    pub num: String,
    pub title_path: Option<String>,
    pub texte: String,
    /// The days its amendment notes say the amendments took effect, as `YYYY-MM-DD`.
    pub amended: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JustelPage {
    pub articles: Vec<JustelArticle>,
    /// How many archived versions a current page links to.
    pub archived: usize,
    /// When an archived version stopped applying.
    pub until: Option<JustelUntil>,
    /// The day the whole text was repealed, when its title says so.
    pub repealed_on: Option<String>,
    /// Whether the page shows the whole text, which a table of contents marks, rather than only the
    /// articles the next amendment changed.
    pub whole: bool,
    /// The document a page sends to for its text, `voir 2013-05-02/09`, as Justel numbers it.
    pub elsewhere: Option<String>,
    /// Why a page that shows no article shows none, when it says so.
    pub no_text: Option<JustelNoText>,
}

/// Why a page shows no article.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustelNoText {
    /// `(NOTE : pas de texte disponible)`, `(Texte non repris pour des raisons techniques ...)`.
    Unavailable,
    /// `(NOTE : texte indisponible en format texte, voir image)`.
    Image,
    /// `[Abrogé]`, `(abrogée) <...>`, `(remplacé) <...>`: the text was repealed whole, and is no
    /// longer shown.
    Repealed,
    /// `[Retiré]`, `(Rapporté) <...>`: the act was withdrawn.
    Withdrawn,
    /// An erratum declares the publication void, or `(annulé et remplacé par ...)`.
    Void,
    /// A circular, written without articles.
    Circular,
    /// A notice, an erratum or an entry into force, under its mention and annex alone.
    Notice,
    Empty,
}

/// When an archived version stopped applying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JustelUntil {
    /// The first day of the next version, as `YYYY-MM-DD`.
    Day(String),
    /// `En vigueur jusqu'au indéterminée`.
    Undetermined,
}

static ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<A NAME='(Art\.[^']*|Article[^']*|LNK\d+)'[^>]*>").unwrap());
static ARCHIVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)\s+(?:versions\s+archiv|gearchiveerde\s+versies)").unwrap());
static UNTIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)en vigueur jusqu.au\s*:?\s*(\d{2})-(\d{2})-(\d{4})").unwrap()
});
static UNDETERMINED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)en vigueur jusqu.au\s*:?\s*ind[ée]termin[ée]e").unwrap());
/// The note a page shows in place of its text: `(NOTE : pour le texte voir <A HREF=…cn_search=2013050209…>`.
static ELSEWHERE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*\(NOTE\s*:[^<]*voir\s*<A HREF=[^>]*cn_search=(\d+)").unwrap()
});
static REPEALED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\(Abrogé par [^)]*?En vigueur\s*:\s*(\d{2})-(\d{2})-(\d{4})\)").unwrap()
});
/// The line that opens an article's wording not in force yet.
static FUTURE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)DROIT FUTUR\s*<br").unwrap());
static FOOTNOTES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-{6,}").unwrap());
/// An amendment note repealing the article, `<Abrogé par …>`, `<Opgeheven bij …>`.
static ABROGATION_NOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)&lt;\s*(abrog|opgeh)").unwrap());
static ITALIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<i>.*?</i>").unwrap());
static TOOLTIP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<a href=#t title='[^']*'>.*?</a>").unwrap());
static TOOLTIP_NOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<a href=#t title='<([^']*)>'>").unwrap());
static SUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<sup>.*?</sup>").unwrap());
static STYLE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(sup|font|span|b|i)\b[^>]*>").unwrap());
static NOTE_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)&lt;(.*?)&gt;").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</?a\b[^>]*>").unwrap());
static BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:Article|Artikel|Art\.)\s*").unwrap());
static HEAD_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:er|ère)?\s*\.?").unwrap());
/// An amendment note whose opening sign the source lost: `L 2001-04-29/39, art. 20, 013; ...>`.
static OPEN_NOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<?\s*\b(?:L|AL|AR|LW|W)\s+\d[\d/-]*,\s*art\.[^<>\n]*>").unwrap());
/// An editor's note, or the wrapper an amendment note leaves once its reference is gone.
static EDITORIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\(NOT[EA]\s*:[^()]*\)|\(\s*(?:Inséré|Modifié|Remplacé|Ajouté|Complété|Rétabli|Abrogé|Confirmé)\s+par\s*\)|\(\s*(?:Ingevoegd|Gewijzigd|Vervangen|Aangevuld|Hersteld|Opgeheven|Bekrachtigd)\s+bij\s*\)",
    )
    .unwrap()
});
static IN_FORCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:en vigueur|inwerkingtreding|\bED)\s*:\s*(?:(\d{2})-(\d{2})-(\d{4})|(\d{4})-(\d{2})-(\d{2}))")
        .unwrap()
});
/// The head of a text's sole article: `Article unique.`, `Enig artikel -`.
static SOLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:Article unique|Enig artikel|Einziger Artikel)\s*\.?\s*(?:-\s*)?").unwrap()
});
/// What follows the last article: the promulgation, the place and day of signature, or the
/// parliamentary documents' list.
static CLOSING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:Promulguons|Kondigen|Session \d{4}-\d{4}$|(?:Donné à |Gegeven te )?(?:Eupen|Bruxelles|Brussel|Namur|Namen), (?:le )?\d)").unwrap()
});
static ELLIPSIS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*(?:\.{2,}|…)\s*$").unwrap());

/// `YYYY-MM-DD` from day, month and year, when they make a date.
fn iso(day: &str, month: &str, year: &str) -> Option<String> {
    let date = jiff::civil::Date::new(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    date.ok().map(|d| d.to_string())
}

/// The visible text of an HTML fragment, its tags and amendment notes gone, one line per break.
fn plain(html: &str) -> String {
    let s = html.replace("&LT;", "&lt;").replace("&GT;", "&gt;");
    let s = ITALIC.replace_all(&s, |c: &Captures| {
        if c[0].contains("(NOTE") || c[0].contains("(NOTA") {
            String::new()
        } else {
            c[0].to_string()
        }
    });
    let s = TOOLTIP.replace_all(&s, "");
    let s = SUP.replace_all(&s, "");
    let s = STYLE_TAG.replace_all(&s, "");
    let s = NOTE_BLOCK.replace_all(&s, "");
    let s = LINK.replace_all(&s, "");
    let s = BREAK.replace_all(&s, "\n");
    let s = TAG.replace_all(&s, "");
    html_escape::decode_html_entities(&s).replace('\u{a0}', " ")
}

fn lines(text: &str) -> String {
    text.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The days the amendment notes of a segment name, in its notes and its tooltips.
fn amended(segment: &str) -> Vec<String> {
    let s = segment.replace("&LT;", "&lt;").replace("&GT;", "&gt;");
    let notes = NOTE_BLOCK
        .captures_iter(&s)
        .chain(TOOLTIP_NOTE.captures_iter(&s))
        .map(|c| html_escape::decode_html_entities(&TAG.replace_all(&c[1], " ")).into_owned());
    let mut days: Vec<String> = notes
        .flat_map(|note| {
            IN_FORCE
                .captures_iter(&note)
                .filter_map(|c| match (c.get(1), c.get(4)) {
                    (Some(_), _) => iso(&c[1], &c[2], &c[3]),
                    (None, Some(_)) => iso(&c[6], &c[5], &c[4]),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    days.sort();
    days.dedup();
    days
}

/// The text after an article's heading, `Art. 41.` before `41.Les intérêts` included.
fn unheaded<'a>(text: &'a str, num: &str) -> &'a str {
    let Some(head) = HEAD.find(text) else {
        return text;
    };
    let rest = &text[head.end()..];
    match rest.strip_prefix(num) {
        Some(rest) => &rest[HEAD_END.find(rest).map_or(0, |m| m.end())..],
        None => text,
    }
}

/// An article's body: its text before its footnotes and any future wording, without its heading.
fn body(segment: &str, num: &str) -> String {
    let text = FOOTNOTES.split(segment).next().unwrap_or_default();
    let repealed = ABROGATION_NOTE
        .captures(&text.replace("&LT;", "&lt;"))
        .map(|c| match c[1].to_lowercase().as_str() {
            "opgeh" => "(Opgeheven)",
            _ => "(Abrogé)",
        });
    let text = plain(text);
    let text = OPEN_NOTE.replace_all(unheaded(&text, num), "");
    let text = lines(&EDITORIAL.replace_all(&text, ""));
    match repealed {
        Some(repealed) if text.is_empty() => repealed.to_string(),
        _ => text,
    }
}

fn heading_label(html: &str) -> String {
    let label = plain(html).replace(['[', ']'], "").replace('>', "›");
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    ELLIPSIS.replace(&label, "").into_owned()
}

/// The rank of a division by the word it opens on, deepest last.
fn level(label: &str) -> Option<u8> {
    let low = label.trim_start_matches(['[', '(', ' ']).to_lowercase();
    if low
        .split_whitespace()
        .take(2)
        .any(|w| w == "partie" || w == "deel")
    {
        return Some(1);
    }
    [
        ("livre", 2),
        ("boek", 2),
        ("titre", 3),
        ("titel", 3),
        ("sous-titre", 4),
        ("ondertitel", 4),
        ("chapitre", 5),
        ("hoofdstuk", 5),
        ("section", 6),
        ("afdeling", 6),
        ("sous-section", 7),
        ("onderafdeling", 7),
        ("paragraphe", 8),
        ("paragraaf", 8),
        ("§", 8),
    ]
    .into_iter()
    .find(|(word, _)| low.starts_with(word))
    .map(|(_, rank)| rank)
}

/// The divisions open at each point of a text. A heading outside the usual words sits under the
/// last known one and beside an unknown one before it, and the next known heading closes it.
#[derive(Default)]
struct Divisions {
    stack: Vec<(Option<u8>, String)>,
}

impl Divisions {
    fn open(&mut self, label: String) {
        let rank = level(&label);
        while let Some((top, _)) = self.stack.last() {
            let closes = match (top, rank) {
                (None, _) => true,
                (Some(top), Some(rank)) => *top >= rank,
                (Some(_), None) => false,
            };
            if !closes {
                break;
            }
            self.stack.pop();
        }
        self.stack.push((rank, label));
    }

    fn path(&self) -> Option<String> {
        (!self.stack.is_empty()).then(|| {
            self.stack
                .iter()
                .map(|(_, l)| l.as_str())
                .collect::<Vec<_>>()
                .join(" > ")
        })
    }
}

/// A Justel page, decoded. An article shown twice keeps its first showing; an anchor whose number
/// holds no digit is a note or an annex, and one marked as future law is not in force yet.
pub fn justel_page(html: &str) -> Result<JustelPage, String> {
    let start = html.find("id=\"text\"").ok_or("the page holds no text")?;
    let start = start + html[start..].find("</h2>").map_or(0, |i| i + 5);
    let end = html[start..]
        .find("<div id=\"list-title-")
        .map_or(html.len(), |i| start + i);
    let (text, after) = (&html[start..end], &html[end..]);
    let title = html.find("id=\"list-title-1\"").map(|i| {
        &html[i..html[i..]
            .find("id=\"list-title-2\"")
            .map_or(start, |j| i + j)]
    });

    let anchors: Vec<_> = ANCHOR.captures_iter(text).collect();
    let mut divisions = Divisions::default();
    let mut articles: Vec<JustelArticle> = Vec::new();
    for (i, anchor) in anchors.iter().enumerate() {
        let whole = anchor.get(0).unwrap();
        let next = anchors
            .get(i + 1)
            .map_or(text.len(), |a| a.get(0).unwrap().start());
        let segment = &text[whole.end()..next];
        let segment = &segment[..FUTURE.find(segment).map_or(segment.len(), |m| m.start())];
        let name = &anchor[1];
        if name.starts_with("LNK") {
            let line = segment
                .find("<BR>")
                .or_else(|| segment.find("<br>"))
                .map_or(segment, |j| &segment[..j]);
            let label = heading_label(line);
            if label.chars().any(char::is_alphanumeric) {
                divisions.open(label);
            }
            continue;
        }
        let label = match name.get(.."Article".len()) {
            Some(p) if p.eq_ignore_ascii_case("Article") => &name["Article".len()..],
            _ => &name["Art.".len()..],
        };
        let label = label.trim().trim_end_matches('.').trim();
        // `Art.Article` names no number: it heads a sole article, its number shown or not.
        let bare = label.eq_ignore_ascii_case("article");
        let label = if bare {
            let text = plain(segment);
            let rest = HEAD.find(&text).map_or("", |head| &text[head.end()..]);
            rest.chars().take_while(char::is_ascii_digit).collect()
        } else {
            label.to_string()
        };
        let num = match label.as_str() {
            "premier" => "1er",
            _ if label.eq_ignore_ascii_case("unique") => "unique",
            _ => &label,
        };
        if !(bare || num.contains(|c: char| c.is_ascii_digit()) || num == "unique")
            || num.contains("DROIT FUTUR")
            || articles.iter().any(|a| a.num == num)
        {
            continue;
        }
        let texte = body(segment, &label);
        if texte.is_empty() {
            continue;
        }
        let (num, texte) = match SOLE.find(&texte) {
            Some(head) if bare || num == "1" => ("unique", texte[head.end()..].to_string()),
            _ if bare => continue,
            _ if num == "unique" => (num, texte.trim_start_matches("- ").to_string()),
            _ => (num, texte),
        };
        articles.push(JustelArticle {
            num: num.to_string(),
            title_path: divisions.path(),
            texte,
            amended: amended(segment),
        });
    }
    if anchors.is_empty() && SOLE.is_match(plain(text).trim_start()) {
        articles.push(JustelArticle {
            num: "unique".to_string(),
            title_path: None,
            texte: SOLE
                .replace(&body(text, "unique"), "")
                .trim_start_matches("- ")
                .to_string(),
            amended: amended(text),
        });
    }
    if let Some(last) = articles.last_mut() {
        let mut kept: Vec<&str> = last
            .texte
            .lines()
            .take_while(|l| !CLOSING.is_match(l))
            .collect();
        if kept.len() < last.texte.lines().count() {
            while kept.last().is_some_and(|l| l.starts_with("Adopté par")) {
                kept.pop();
            }
            last.texte = kept.join("\n");
        }
    }

    let date = |re: &Regex, s: &str| {
        re.captures(&plain(s))
            .and_then(|c| iso(&c[1], &c[2], &c[3]))
    };
    Ok(JustelPage {
        archived: ARCHIVED
            .captures(html)
            .and_then(|c| c[1].parse().ok())
            .unwrap_or(0),
        until: date(&UNTIL, after).map(JustelUntil::Day).or_else(|| {
            UNDETERMINED
                .is_match(&plain(after))
                .then_some(JustelUntil::Undetermined)
        }),
        repealed_on: title.and_then(|t| date(&REPEALED, t)),
        whole: html.contains("id=\"list-title-sw_t\""),
        elsewhere: ELSEWHERE.captures(text).map(|c| c[1].to_string()),
        no_text: if articles.is_empty() {
            no_text(text, title)
        } else {
            None
        },
        articles,
    })
}

fn no_text(text: &str, title: Option<&str>) -> Option<JustelNoText> {
    let shown = plain(&text[..text.find("</div>").unwrap_or(text.len())]);
    let shown = shown.trim().to_lowercase();
    let opening: String = shown.chars().take(60).collect();
    if shown.is_empty() {
        Some(JustelNoText::Empty)
    } else if shown.contains("pas de texte disponible") || shown.contains("texte non repris") {
        Some(JustelNoText::Unavailable)
    } else if shown.contains("texte indisponible en format texte") {
        Some(JustelNoText::Image)
    } else if shown.contains("comme nulle") || opening.starts_with("(annulé") {
        Some(JustelNoText::Void)
    } else if opening.contains("abrog")
        || opening.starts_with("(abogé")
        || opening.starts_with("(remplacé")
    {
        Some(JustelNoText::Repealed)
    } else if opening.starts_with("[retiré") || opening.starts_with("(rapporté") {
        Some(JustelNoText::Withdrawn)
    } else if title.is_some_and(|t| plain(t).to_lowercase().contains("- circulaire")) {
        Some(JustelNoText::Circular)
    } else if text.contains("NAME='Art.M'") {
        Some(JustelNoText::Notice)
    } else {
        None
    }
}

/// An act a Justel list shows, its days as `YYYY-MM-DD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JustelListed {
    pub numac: String,
    /// `Région de Bruxelles-Capitale`, `Commission communautaire commune`.
    pub issuer: String,
    /// Its title without the day it opens on: `Ordonnance relative à …`.
    pub heading: String,
    pub adopted: Option<String>,
    pub published: Option<String>,
}

/// One page of a Justel search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JustelList {
    pub acts: Vec<JustelListed>,
    /// The number of its last page.
    pub pages: usize,
    /// Whether Justel refused the search for naming more than it lists.
    pub capped: bool,
}

/// An act Justel consolidated again, as its feed of recent consolidations shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JustelConsolidated {
    /// The day of the consolidation.
    pub day: String,
    pub adopted: Option<String>,
    pub heading: String,
}

static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<p class="list-item-{2}subtitle">(.*?)</p>\s*<a href="[^"]*numac_search=([0-9A-Za-z]+)[^"]*" class="list-item-{2}title">(.*?)</a>\s*<p class="list-item-{2}date">(.*?)</p>"#,
    )
    .unwrap()
});
static LAST_PAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"[?&]page=(\d+)[^"]*" class="pagination-button pagination-last""#).unwrap()
});
static FEED_DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<h2 class="list-title"[^>]*>(.*?)</h2>"#).unwrap());
static FEED_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<a href="article\.pl\?[^"]*cn_search=\d+[^"]*" class="list-item-{2}title">(.*?)</a>"#,
    )
    .unwrap()
});

fn day_fr(text: &str) -> Option<String> {
    lj_core::dates::parse_date_fr(&text.replace('\u{a0}', " ")).map(|d| d.to_string())
}

/// `27 avril 2000. - Ordonnance portant ….` as its day and its heading, without the stops it ends
/// on.
fn dated_heading(title: &str) -> (Option<String>, String) {
    let title = lines(&plain(title)).replace('\n', " ");
    let (day, heading) = match title.split_once(". - ") {
        Some((day, heading)) if day_fr(day).is_some() => (day_fr(day), heading),
        _ => (None, title.as_str()),
    };
    let heading = heading.trim_end_matches(['.', ' ', '-']).trim_start();
    (day, heading.to_string())
}

/// The acts a page of a Justel search lists, and how many pages it has.
pub fn justel_list(html: &str) -> JustelList {
    let acts = LIST_ITEM
        .captures_iter(html)
        .map(|c| {
            let (adopted, heading) = dated_heading(&c[3]);
            JustelListed {
                numac: c[2].to_string(),
                issuer: lines(&plain(&c[1])),
                heading,
                adopted,
                published: day_fr(&lines(&plain(&c[4]))),
            }
        })
        .collect::<Vec<_>>();
    JustelList {
        pages: LAST_PAGE
            .captures(html)
            .and_then(|c| c[1].parse().ok())
            .unwrap_or(usize::from(!acts.is_empty())),
        capped: html.contains("plus de 10.000"),
        acts,
    }
}

/// The acts Justel's feed of recent consolidations names, newest consolidation first.
pub fn justel_consolidated(html: &str) -> Vec<JustelConsolidated> {
    let days: Vec<(usize, String)> = FEED_DAY
        .captures_iter(html)
        .filter_map(|c| Some((c.get(0)?.start(), day_fr(&plain(&c[1]))?)))
        .collect();
    FEED_ITEM
        .captures_iter(html)
        .filter_map(|c| {
            let at = c.get(0)?.start();
            let (_, day) = days.iter().rev().find(|(start, _)| *start < at)?;
            let (adopted, heading) = dated_heading(&c[1]);
            Some(JustelConsolidated {
                day: day.clone(),
                adopted,
                heading,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str, after: &str) -> String {
        format!(
            "<div id=\"list-title-1\" class=\"box\">Titre 8 JUIN 1867. - CODE PENAL. (Abrogé par 2026-03-03/04, art. 129,4°; En vigueur : 01-09-2026)</div>\
             <div id=\"list-title-2\"><A NAME='LNKR0001' HREF='#LNK0001'>TITRE Ier.</A></div>\
             <div id=\"list-title-3\"><h2 id=\"text\" class=\"list-title\">Texte</h2>{text}</div>\
             <div id=\"list-title-sw_t\" class=\"box\">Table des matières</div>\
             <div id=\"list-title-sw_roi\" class=\"box\">{after}</div>"
        )
    }

    #[test]
    fn an_article_reads_without_its_notes_markers_and_heading() {
        let html = page(
            "<A NAME='LNK0001' HREF='#LNKR0001'>TITRE Ier.</A> - Des peines.<BR>\
             <A NAME='LNK0002' HREF='#LNKR0002'>CHAPITRE II.</A> [<sup><font color=red>1</font></sup> Des amendes]<sup><font color=red>1</font></sup><BR>\
             <A NAME='Art.7bis' HREF='#Art.7'>Art.</A> <A HREF='#Art.8'> 7bis</A>.<BR>&nbsp;&nbsp;[<sup><font color=red><a href=#t title='<Inséré par L 2016-02-05/11, art. 2, 152; En vigueur : 29-02-2016>'><span style='color:red;'>1</span></a></font></sup> Les peines<sup>2</sup> sont :]<sup><font color=red>1</font></sup><BR>&nbsp;&nbsp;1° l'amende;<BR>&nbsp;&nbsp;----------<BR>&nbsp;&nbsp;(1)&lt;L <A HREF=x>2016-02-05/11</A>, art. 2, 152; En vigueur : 29-02-2016&gt;<BR>\
             <A NAME='Art.8' HREF='#Art.7bis'>Art.</A> 8. Texte ancien. &LT;L 1990-07-17/30, art. 4, 004; <b> En vigueur : </b> 1990-08-25&GT; <BR>",
            "",
        );
        let p = justel_page(&html).unwrap();
        assert_eq!(p.articles.len(), 2);
        let a = &p.articles[0];
        assert_eq!(a.num, "7bis");
        assert_eq!(
            a.title_path.as_deref(),
            Some("TITRE Ier. - Des peines. > CHAPITRE II. Des amendes")
        );
        assert_eq!(a.texte, "[ Les peines sont :]\n1° l'amende;");
        assert_eq!(a.amended, vec!["2016-02-29"]);
        assert_eq!(p.articles[1].texte, "Texte ancien.");
        assert_eq!(p.articles[1].amended, vec!["1990-08-25"]);
        assert_eq!(p.repealed_on.as_deref(), Some("2026-09-01"));
        assert!(p.whole);
    }

    #[test]
    fn a_repealed_article_says_so_and_future_law_is_left_out() {
        let html = page(
            "<A NAME='Art.1134' HREF='#Art.1133'>Art.</A> <A HREF='#Art.1135'> 1134</A>.<BR>&nbsp;&nbsp;&lt;Abrogé par L <A HREF=x>2022-04-28/25</A>, art. 62, 048; En vigueur : 01-01-2023&gt; <BR>\
             <A NAME='Art.1135' HREF='#Art.1134'>Art.</A> 1135. En vigueur.<BR>&nbsp;&nbsp;DROIT FUTUR<BR><i>Art. 1135. Futur. &lt;L 2026-10-08/1, art. 3, 160; En vigueur : 01-01-2027&gt;</i><BR>\
             <A NAME='Art.1136 DROIT FUTUR..' HREF='#Art.1135'>Art.1136</A> DROIT FUTUR. Futur.<BR>\
             <A NAME='Art.N' HREF='#Art.1135'>Art.</A> N. Annexe.<BR>",
            "<br>Modifi&eacute; par LOI du 15-07-2026 Art. 195 En vigueur jusqu'au&nbsp;&nbsp; 24-07-2026 Voir texte ci-dessus",
        );
        let p = justel_page(&html).unwrap();
        let nums: Vec<_> = p.articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(nums, ["1134", "1135"]);
        assert_eq!(p.articles[0].texte, "(Abrogé)");
        assert_eq!(p.articles[0].amended, vec!["2023-01-01"]);
        assert_eq!(p.articles[1].texte, "En vigueur.");
        assert!(p.articles[1].amended.is_empty());
        assert_eq!(p.until, Some(JustelUntil::Day("2026-07-24".to_string())));
        assert_eq!(p.elsewhere, None);
    }

    #[test]
    fn a_page_names_the_document_holding_its_text_and_an_undetermined_end() {
        let html = page(
            "\n (NOTE : pour le texte voir <A HREF=/cgi_loi/article.pl?language=fr&amp;lg_txt=f&amp;cn_search=2013050209 target=_blank>2013-05-02/09</A> )",
            "<b>En vigueur jusqu'au&nbsp;&nbsp;</b>  ind&eacute;termin&eacute;e",
        );
        let p = justel_page(&html).unwrap();
        assert!(p.articles.is_empty());
        assert_eq!(p.elsewhere.as_deref(), Some("2013050209"));
        assert_eq!(p.until, Some(JustelUntil::Undetermined));
    }

    #[test]
    fn a_page_without_articles_says_why_and_an_article_anchor_reads_as_one() {
        let why = |text: &str| justel_page(&page(text, "")).unwrap().no_text;
        assert_eq!(
            why("\n (NOTE : pas de texte disponible)<BR>&nbsp;&nbsp;"),
            Some(JustelNoText::Unavailable)
        );
        assert_eq!(
            why("\n (NOTE : texte indisponible en format texte, voir image)<BR>"),
            Some(JustelNoText::Image)
        );
        assert_eq!(
            why("\n [Abrogé] &lt;ORD <A HREF=x>2023-07-20/10</A>, art. 119, 011; En vigueur : 24-08-2023&gt;"),
            Some(JustelNoText::Repealed)
        );
        assert_eq!(
            why("<A NAME='Art.M'>Article </A> <A HREF='#Art.N'>M</A>. Dans la publication du 4 juin 2024, lire :<BR>"),
            Some(JustelNoText::Notice)
        );
        assert_eq!(
            why("\n   \n </div><div class=\"links\">Liens</div>"),
            Some(JustelNoText::Empty)
        );
        assert_eq!(
            why("\n (abrogée) &lt;ORD <A HREF=x>2004-04-01/49</A>, art. 10; En vigueur : 06-05-2004&gt;"),
            Some(JustelNoText::Repealed)
        );
        assert_eq!(
            why("\n (coordonnée dans COBAT 2004-04-09/35 et abrogée) &lt;ORD 2004-05-13/31&gt;"),
            Some(JustelNoText::Repealed)
        );
        assert_eq!(
            why("\n [Retiré] &lt;ARR 2015-07-02/37, art. 1, 002; En vigueur : 01-01-2014&gt;"),
            Some(JustelNoText::Withdrawn)
        );
        assert_eq!(
            why("\n (NOTE : La publication de cet arrêté doit être considérée comme nulle, ERRATUM)"),
            Some(JustelNoText::Void)
        );
        assert_eq!(
            why("\n (Texte non repris pour des raisons techniques, voir M.B. du 08-02-2010)"),
            Some(JustelNoText::Unavailable)
        );
        assert_eq!(
            why("\n (Abogé) &lt;ARR 1999-05-06/52, art.407, 004; En vigueur : 01-07-1999&gt;"),
            Some(JustelNoText::Repealed)
        );
        assert_eq!(
            why("\n (remplacé) &lt;ARR 2002-06-27/36, art. 3, 002; En vigueur : 01-09-2002&gt;"),
            Some(JustelNoText::Repealed)
        );
        assert_eq!(
            why("\n (Rapporté) &lt;ARR 2002-09-26/50, art. 4, 002; En vigueur : 1999-07-01&gt;"),
            Some(JustelNoText::Withdrawn)
        );
        assert_eq!(
            why("\n (annulé et remplacé par ARR 1998-12-10/38)<BR>"),
            Some(JustelNoText::Void)
        );
        assert_eq!(why("\n Ceci est un texte.<BR>"), None);
        let circular = page("\n Chers collègues,<BR>", "").replace(
            "8 JUIN 1867. - CODE PENAL.",
            "26 SEPTEMBRE 2022. - Circulaire ministérielle sur la dispense de stage.",
        );
        assert_eq!(
            justel_page(&circular).unwrap().no_text,
            Some(JustelNoText::Circular)
        );

        let p = justel_page(&page(
            "<A NAME='Art.M'>Article </A> <A HREF='#Articlepremier'>M</A>. <BR>&nbsp;&nbsp;L'Assemblée a adopté :<BR><BR>\
             &nbsp;&nbsp;<A NAME='Articlepremier' HREF='#Art.M'>Article</A> <A HREF='#Article2'> premier</A><BR>&nbsp;&nbsp;Le présent décret règle une matière communautaire.<BR><BR>\
             &nbsp;&nbsp;<A NAME='Article2' HREF='#Articlepremier'>Article</A> <A HREF='#Art.N'> 2</A><BR>&nbsp;&nbsp;Le Protocole sortira son plein et entier effet.<BR>",
            "",
        ))
        .unwrap();
        let read: Vec<_> = p
            .articles
            .iter()
            .map(|a| (a.num.as_str(), a.texte.as_str()))
            .collect();
        assert_eq!(
            read,
            [
                ("1er", "Le présent décret règle une matière communautaire."),
                ("2", "Le Protocole sortira son plein et entier effet.")
            ]
        );
        assert_eq!(p.no_text, None);
    }

    #[test]
    fn an_unknown_heading_sits_under_the_known_one_until_the_next_known_one() {
        let mut d = Divisions::default();
        for label in [
            "PREMIERE PARTIE",
            "LIVRE III.",
            "TITRE V.",
            "DISPOSITIONS COMMUNES",
            "A. Règles",
        ] {
            d.open(label.to_string());
        }
        assert_eq!(
            d.path().as_deref(),
            Some("PREMIERE PARTIE > LIVRE III. > TITRE V. > A. Règles")
        );
        d.open("CHAPITRE I.".to_string());
        assert_eq!(
            d.path().as_deref(),
            Some("PREMIERE PARTIE > LIVRE III. > TITRE V. > CHAPITRE I.")
        );
        d.open("TITRE VI.".to_string());
        assert_eq!(
            d.path().as_deref(),
            Some("PREMIERE PARTIE > LIVRE III. > TITRE VI.")
        );
    }

    #[test]
    fn a_list_names_each_act_its_issuer_day_and_heading_and_its_last_page() {
        let html = include_str!("../tests/fixtures/justel_list.html");
        let list = justel_list(html);
        assert_eq!(list.pages, 69);
        assert!(!list.capped);
        assert_eq!(
            list.acts,
            [JustelListed {
                numac: "2004A31182".to_string(),
                issuer: "Région de Bruxelles-Capitale".to_string(),
                heading: "Code bruxellois de l'aménagement du territoire (CoBAT)".to_string(),
                adopted: Some("2004-04-09".to_string()),
                published: Some("2004-05-26".to_string()),
            }]
        );
        assert!(justel_list("Vos critères renvoient plus de 10.000 articles.").capped);
    }

    #[test]
    fn the_feed_dates_each_act_by_the_day_it_was_consolidated() {
        let html = include_str!("../tests/fixtures/justel_consolidated.html");
        assert_eq!(
            justel_consolidated(html),
            [
                JustelConsolidated {
                    day: "2026-11-18".to_string(),
                    adopted: Some("2019-02-07".to_string()),
                    heading: "Arrêté du Gouvernement fixant les branches".to_string(),
                },
                JustelConsolidated {
                    day: "2026-11-17".to_string(),
                    adopted: Some("1995-03-30".to_string()),
                    heading: "Décret relatif à la publicité de l'Administration".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_dutch_page_reads_its_heads_amendments_repeals_and_archives() {
        let p = justel_page(&page(
            "<A NAME='LNK0002' HREF='#LNKR0002'>TITEL I.</A> - HET FEDERALE BELGIE.<BR>\
             <A NAME='Art.1'></A>Artikel <A HREF='#Art.2'> 1</A>. België is een federale Staat.<BR><BR>\
             &nbsp;&nbsp;<A NAME='Art.2' HREF='#Art.1'>Art.</A> <A HREF='#Art.3'> 2</A>.(De gelijkheid van vrouwen en mannen is gewaarborgd.) &lt;W 2002-02-21/33, art. 1, 015; <b> Inwerkingtreding : </b> 26-02-2002&gt;<BR><BR>\
             &nbsp;&nbsp;<A NAME='Art.3' HREF='#Art.2'>Art.</A> <A HREF='#Art.4'> 3</A>. &lt;Opgeheven bij W 2005-12-06/47, art. 1, 021; <b> Inwerkingtreding : </b> 28-12-2005&gt;<BR>",
            "<a href=x>79 gearchiveerde versies </a>",
        ))
        .unwrap();
        let read: Vec<_> = p
            .articles
            .iter()
            .map(|a| {
                (
                    a.num.as_str(),
                    a.title_path.as_deref(),
                    a.texte.as_str(),
                    a.amended.clone(),
                )
            })
            .collect();
        let path = Some("TITEL I. - HET FEDERALE BELGIE.");
        assert_eq!(
            read,
            [
                ("1", path, "België is een federale Staat.", vec![]),
                (
                    "2",
                    path,
                    "(De gelijkheid van vrouwen en mannen is gewaarborgd.)",
                    vec!["2002-02-26".to_string()]
                ),
                ("3", path, "(Opgeheven)", vec!["2005-12-28".to_string()]),
            ]
        );
        assert_eq!(p.archived, 79);
    }

    #[test]
    fn a_sole_article_reads_as_unique_and_the_promulgation_after_the_last_article_is_left_out() {
        let read = |text: &str| -> Vec<(String, String)> {
            justel_page(&page(text, ""))
                .unwrap()
                .articles
                .into_iter()
                .map(|a| (a.num, a.texte))
                .collect()
        };
        let sole = |texte: &str| vec![("unique".to_string(), texte.to_string())];
        assert_eq!(
            read("<A NAME='Art.1'></A>Article 1. Article unique. L'Accord sortira son plein et entier effet.<BR>&nbsp;&nbsp;<BR>&nbsp;&nbsp;Session 2021-2022<BR>&nbsp;&nbsp;Documents parlementaires : 163 (2021-2022) n° 1 Projet de décret"),
            sole("L'Accord sortira son plein et entier effet.")
        );
        assert_eq!(
            read("Le Parlement a adopté et Nous, Gouvernement, sanctionnons ce qui suit :<BR><BR>&nbsp;&nbsp;<A NAME='Art.unique'>Article </A> <A HREF='#LNK0001'>unique</A>. Le Protocole sortira son plein et entier effet.<BR>&nbsp;&nbsp;Promulguons le présent décret et ordonnons qu'il soit publié au Moniteur belge.<BR>&nbsp;&nbsp;Eupen, le 19 janvier 2009."),
            sole("Le Protocole sortira son plein et entier effet.")
        );
        assert_eq!(
            read("\n Article unique. - La Convention sortira son plein et entier effet.<BR>&nbsp;&nbsp;Eupen, le 14 mars 1994.<BR>&nbsp;&nbsp;J. MARAITE<BR>"),
            sole("La Convention sortira son plein et entier effet.")
        );
        assert_eq!(
            read("<A NAME='Art.Article'>Article1.</A> Article unique. L'article 3 est modifié.<BR>&nbsp;&nbsp;Bruxelles, le 8 mars 2002.<BR>&nbsp;&nbsp;Pour le Gouvernement de la Communauté française :<BR>"),
            sole("L'article 3 est modifié.")
        );
        assert_eq!(
            read("<A NAME='Art.Article'> </A> <A HREF='#LNK0001'>Article</A>. Article unique.<BR>&nbsp;&nbsp; - Assentiment est donné à l'accord.<BR>\
                  &nbsp;&nbsp;<A NAME='LNK0001' HREF='#LNKR0001'>ANNEXE.</A><BR><BR>\
                  &nbsp;&nbsp;<A NAME='Art.N' HREF='#Art.Article'>Art.</A> N.<BR>&nbsp;&nbsp;(Image non reprise pour des raisons techniques)"),
            sole("Assentiment est donné à l'accord.")
        );
        assert_eq!(
            read("<A NAME='Art.1' HREF='#Art.2'>Art.</A> <A HREF='#Art.2'> 1</A>. Premier.<BR><BR>\
                  <A NAME='Art.2' HREF='#Art.1'>Art.</A> <A HREF='#Art.3'> 2</A>. Les arrêtés sont abrogés.<BR>\
                  Adopté par le Conseil de la Région de Bruxelles-Capitale.<BR>\
                  Promulguons la présente ordonnance, ordonnons qu'elle soit publiée au Moniteur belge.<BR>\
                  Donné à Bruxelles, le 8 avril 1993.<BR>"),
            vec![
                ("1".to_string(), "Premier.".to_string()),
                ("2".to_string(), "Les arrêtés sont abrogés.".to_string())
            ]
        );
    }
}
