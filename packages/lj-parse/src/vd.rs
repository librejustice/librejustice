//! The Vaud systematic collection (BLV): the articles of one version of an act, read from the
//! Akoma Ntoso rendering its publication serves.

use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::ConsolidatedArticle;

/// A modification's history, a popover beside the heading it annotates.
static HISTORY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<div class="akn-modifiants-container[^"]*"[^>]*>.*?</table>\s*</div>"#)
        .unwrap()
});
/// The modification's call, `[ 7 ]`, after a heading.
static HISTORY_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<span class="akn-modifiant-number"[^>]*>\s*\[\s*<span[^>]*>.*?</span>\s*\]\s*</span>"#)
        .unwrap()
});
/// The file name an image's caption prints under the image.
static IMAGE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<p class="akn-image-name[^"]*"[^>]*>.*?</p>"#).unwrap());
static CONTAINER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<div class="akn-([a-zA-Z]+)-container" position="([^"]*)"[^>]*>"#).unwrap()
});
static NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<td class="akn-num[^"]*"[^>]*>(.*?)</td>"#).unwrap());
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<div class="akn-heading[ "][^>]*>(.*?)</div>"#).unwrap());
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</p>|</div>|</tr>|<br\s*/?>").unwrap());
static CELL_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</td>").unwrap());
/// An inline element leaves its text glued to its neighbours: `1<sup>er</sup>` reads `1er`.
static INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(a|abbr|b|em|i|span|strong|sub|sup|u)\b[^>]*>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

/// The text of an HTML fragment, whitespace folded.
fn plain(html: &str) -> String {
    let html = INLINE_TAG.replace_all(html, "");
    let bare = TAG.replace_all(&html, " ");
    html_escape::decode_html_entities(&bare)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A body one line per paragraph, list item or table row, a row's cells apart.
fn lines(html: &str) -> Vec<String> {
    let html = CELL_END.replace_all(html, "\u{1}");
    BREAK
        .split(&html)
        .map(|line| {
            line.split('\u{1}')
                .map(plain)
                .filter(|c| !c.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// The articles of one version of a BLV act, in document order, each under the titles,
/// chapters and sections its position names. An article with no text has left the act.
pub fn vd_articles(html: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    let start = html
        .find("<akomaNtoso")
        .ok_or("the page carries no Akoma Ntoso document")?;
    let body = &html[start..];
    let end = ["<div class=\"akn-commentaires\"", "</akomaNtoso>"]
        .iter()
        .filter_map(|mark| body.find(mark))
        .min()
        .unwrap_or(body.len());
    let body = HISTORY.replace_all(&body[..end], "");
    let body = HISTORY_CALL.replace_all(&body, "");
    let body = IMAGE_NAME.replace_all(&body, "");
    let containers: Vec<_> = CONTAINER.captures_iter(&body).collect();
    let mut path: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (i, c) in containers.iter().enumerate() {
        let whole = c.get(0).unwrap();
        let until = containers
            .get(i + 1)
            .map_or(body.len(), |next| next.get(0).unwrap().start());
        let segment = &body[whole.end()..until];
        let depth = c[2].split(',').count();
        let num = NUM
            .captures(segment)
            .map(|n| plain(&n[1]))
            .unwrap_or_default();
        let heading = HEADING
            .captures(segment)
            .map(|h| plain(&h[1]))
            .unwrap_or_default();
        if &c[1] != "article" {
            path.truncate(depth - 1);
            let label = [num.as_str(), heading.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            path.push(label.replace('>', "›"));
            continue;
        }
        let Some(num) = num
            .strip_prefix("Art.")
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        let text_start = segment
            .find("</table>")
            .map_or(0, |at| at + "</table>".len());
        let mut text = lines(&segment[text_start..]);
        if text.is_empty() {
            continue;
        }
        if !heading.is_empty() {
            text.insert(0, heading);
        }
        let titles = &path[..path.len().min(depth - 1)];
        out.push(ConsolidatedArticle {
            num: num.to_string(),
            title_path: (!titles.is_empty()).then(|| titles.join(" > ")),
            texte: text.join("\n"),
            nota: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn division(kind: &str, position: &str, num: &str, heading: &str) -> String {
        format!(
            r#"<div class="akn-{kind}-container" position="{position}" eid="x" > <table class="akn-{kind}" akn="{kind}" position="{position}" ><tbody><tr> <td class="akn-num akn-compare" contenteditable="false"><div class="akn-collapsible-icon down"></div>{num}</td> <td class="akn-heading-container"> <div class="akn-heading akn-compare">{heading} </div> </td> </tr></tbody></table> </div>"#
        )
    }

    fn article(position: &str, num: &str, heading: &str, body: &str) -> String {
        format!(
            r#"<div class="akn-article-container" position="{position}" eid="x" > <table class="akn-article"><tbody><tr> <td class="akn-num akn-compare" contenteditable="false"><div class="akn-collapsible-icon down"></div>{num}</td> <td class="akn-heading-container"> <div class="akn-heading akn-compare"> {heading} </div> </td> </tr></tbody></table> </div> <div class="collapse show"> {body} </div>"#
        )
    }

    fn page(parts: &[String]) -> String {
        format!(
            r#"<html><head><script>if (a < b) {{}}</script></head><body><akomaNtoso id="akomaNtoso"> <div class="akn-preface"><div class="akn-docTitle akn-compare">sur la procédure administrative</div></div> {} <div class="akn-commentaires"> <div id="c"><p>Loi du 28.02.1956 sur les communes</p></div> </div></akomaNtoso></body></html>"#,
            parts.concat()
        )
    }

    #[test]
    fn an_article_reads_its_heading_paragraphs_and_points_under_its_divisions() {
        let html = page(&[
            division(
                "chapter",
                "Chapitre I",
                "Chapitre I",
                "Dispositions générales",
            ),
            r#"<div class="collapse show">"#.to_string(),
            division(
                "section",
                "Chapitre I, Section I",
                "Section I",
                "Objet et champ d&apos;application",
            ),
            article(
                "Chapitre I, Section I, Art. 2",
                "Art. 2",
                r#"Champ d&apos;application <span class="akn-modifiant-number" modContainer="m" onclick="toggleModifiants(event, 'm')"> [<span class="akn-modifiant-number-content">&nbsp;7&nbsp;</span>] </span>"#,
                r#"<div class="akn-modifiants-container hide" unique="m"> <span class="close">×</span> <table class="akn-modifiants"> <tbody> <tr><td class="akn-modification-modif"><a>Modif. 7 du 06.11.2018</a></td></tr> </tbody> </table> </div> <div class="akn-alinea akn-compare" position="Chapitre I, Section I, Art. 2, Al. 1" > <sup class="akn-sup-num" contenteditable="false">1</sup> <p>La présente loi s&apos;applique le 1<sup>er</sup> mai</p> <p><img src="data:image/jpeg;base64,AA=="></img></p><p class="akn-image-name id-c" style="text-align:center;">101.01.ecuVD.jpg</p> </div> <table class="akn-list" total="0"> <tbody> <tr class="akn-point" level="0"> <td class="akn-num akn-compare">a.</td> <td class="akn-pointContent akn-compare" colspan="1"><p>à toute décision ;</p></td> </tr> </tbody> </table>"#,
            ),
            "</div>".to_string(),
            division("chapter", "Chapitre II", "Chapitre II", "Procédure"),
            article(
                "Chapitre II, Art. 52a",
                "Art. 52a",
                "",
                r#"<div class="akn-alinea akn-compare"> <sup class="akn-sup-num">1</sup> <p>Reste.</p> </div>"#,
            ),
            article("Chapitre II, Art. 53", "Art. 53", "", ""),
        ]);
        let arts = vd_articles(&html).unwrap();
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].num, "2");
        assert_eq!(
            arts[0].title_path.as_deref(),
            Some("Chapitre I Dispositions générales > Section I Objet et champ d'application")
        );
        assert_eq!(
            arts[0].texte,
            "Champ d'application\n1 La présente loi s'applique le 1er mai\na. à toute décision ;"
        );
        assert_eq!(arts[1].num, "52a");
        assert_eq!(arts[1].title_path.as_deref(), Some("Chapitre II Procédure"));
        assert_eq!(arts[1].texte, "1 Reste.");
    }

    #[test]
    fn a_page_without_its_document_is_refused() {
        assert!(vd_articles("<html><body>Erreur</body></html>").is_err());
    }
}
