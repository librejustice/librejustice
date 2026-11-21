//! LexWork, the platform 19 Swiss cantons publish their systematic collection on: the articles of
//! one version of a cantonal act, read from the structured tree its JSON carries.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::consolidated::ConsolidatedArticle;

static FOOTNOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<a\b[^>]*class=["']footnote["'][^>]*>.*?</a>"#).unwrap());
/// The mark the platform sets on an element a change touched.
static CHANGED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<strong>\s*\*\s*</strong>").unwrap());
static CELL_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</t[dh]>").unwrap());
static ROW_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</tr>").unwrap());
/// An inline element leaves its text glued to its neighbours: `1<sup>er</sup>` reads `1er`.
static INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(a|abbr|b|em|i|span|strong|sub|sup|u)\b[^>]*>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static NUMBER_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<span class=["']number["']>(.*?)</span>"#).unwrap());

/// The text of an HTML fragment, its footnote calls and change marks left out, whitespace folded.
fn plain(html: &str) -> String {
    let html = FOOTNOTE.replace_all(html, "");
    let html = CHANGED.replace_all(&html, "");
    let html = INLINE_TAG.replace_all(&html, "");
    let bare = TAG.replace_all(&html, " ");
    html_escape::decode_html_entities(&bare)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A table one row per line, its cells apart.
fn table_lines(html: &str) -> Vec<String> {
    let html = CELL_END.replace_all(html, "\u{1}");
    ROW_END
        .split(&html)
        .map(|row| {
            row.split('\u{1}')
                .map(plain)
                .filter(|c| !c.is_empty())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .filter(|row| !row.is_empty())
        .collect()
}

fn lang_str<'a>(node: &'a Value, field: &str, lang: &str) -> &'a str {
    node.get(field)
        .and_then(|f| f.get(lang))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A line that says only that what it numbers was repealed: `3 …`, `i) …`.
fn repealed(line: &str) -> bool {
    line.split_whitespace().last() == Some("…")
        && line.split_whitespace().filter(|w| *w != "…").count() <= 1
}

fn children(node: &Value) -> impl Iterator<Item = &Value> {
    node.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn body_lines(node: &Value, lang: &str, out: &mut Vec<String>) {
    let html = lang_str(node, "html_content", lang);
    match node.get("type").and_then(Value::as_str) {
        Some("table") => out.extend(table_lines(html)),
        _ => {
            let line = plain(html);
            if !line.is_empty() {
                out.push(line);
            }
        }
    }
    for child in children(node) {
        body_lines(child, lang, out);
    }
}

fn walk(node: &Value, lang: &str, path: &mut Vec<String>, out: &mut Vec<ConsolidatedArticle>) {
    match node.get("type").and_then(Value::as_str) {
        Some("title") => {
            let label = plain(lang_str(node, "html_content", lang)).replace('>', "›");
            let pushed = !label.is_empty() && !repealed(&label);
            if pushed {
                path.push(label);
            }
            for child in children(node) {
                walk(child, lang, path, out);
            }
            if pushed {
                path.pop();
            }
        }
        Some("article") => {
            let Some(num) = NUMBER_SPAN
                .captures(lang_str(node, "html_content", lang))
                .map(|c| plain(&c[1]))
                .filter(|n| !n.is_empty())
            else {
                return;
            };
            let mut lines = Vec::new();
            for child in children(node) {
                body_lines(child, lang, &mut lines);
            }
            if lines.iter().all(|l| repealed(l)) {
                return;
            }
            let heading = plain(lang_str(node, "text", lang));
            if !heading.is_empty() {
                lines.insert(0, heading);
            }
            out.push(ConsolidatedArticle {
                num,
                title_path: (!path.is_empty()).then(|| path.join(" > ")),
                texte: lines.join("\n"),
                nota: None,
            });
        }
        _ => {}
    }
}

/// The articles of the version a LexWork `show_as_json` answer selects, in `lang`, in document
/// order. An article whose every paragraph is repealed is left out, as having left the act.
pub fn lexwork_articles(payload: &str, lang: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    let answer: Value = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    let root = answer
        .pointer("/text_of_law/selected_version/json_content/document/content")
        .ok_or("the selected version carries no structured content")?;
    let mut out = Vec::new();
    walk(root, lang, &mut Vec::new(), &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(content: Value) -> String {
        serde_json::json!({
            "text_of_law": {"selected_version": {"json_content": {"document": {"content": content}}}}
        })
        .to_string()
    }

    fn node(kind: &str, html: &str, children: Vec<Value>) -> Value {
        serde_json::json!({"type": kind, "html_content": {"fr": html}, "children": children})
    }

    fn article(num: &str, heading: &str, children: Vec<Value>) -> Value {
        serde_json::json!({
            "type": "article",
            "html_content": {"fr": format!(
                "<div class='article'><div class='article_number'><span class='article_symbol'>Art.</span> <span class='number'>{num}</span></div></div>"
            )},
            "text": {"fr": heading},
            "children": children,
        })
    }

    #[test]
    fn an_article_reads_its_heading_paragraphs_and_items_without_notes_nor_marks() {
        let payload = version(node(
            "title",
            "",
            vec![node(
                "title",
                "<div class='level_1 title'><span class='number'>1</span> <span class='title_text'>Dispositions g&eacute;n&eacute;rales</span></div>",
                vec![article(
                    "2a&nbsp;<strong>*</strong>",
                    "Objet et champ d&#39;application",
                    vec![node(
                        "paragraph",
                        "<div class='paragraph'><span class='number'>1</span> <p><span class='text_content'>La loi du 1<sup>er</sup> mai r&egrave;gle<a class=\"footnote\" href=\"#x\">[1]</a> le <a class='law_link'>Grand Conseil</a>:</span></p></div>",
                        vec![node(
                            "enumeration",
                            "<table class='enumeration_item'><tr><td class='number'>a)&nbsp;<strong>*</strong></td><td class='left_col last'>son bureau;</td></tr></table>",
                            vec![],
                        )],
                    )],
                )],
            )],
        ));
        let arts = lexwork_articles(&payload, "fr").unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].num, "2a");
        assert_eq!(
            arts[0].title_path.as_deref(),
            Some("1 Dispositions générales")
        );
        assert_eq!(
            arts[0].texte,
            "Objet et champ d'application\n1 La loi du 1er mai règle le Grand Conseil:\na) son bureau;"
        );
    }

    #[test]
    fn a_table_reads_one_row_per_line() {
        let payload = version(node(
            "title",
            "",
            vec![article(
                "4",
                "",
                vec![node(
                    "table",
                    "<table class='enumeration_tabular'><tr><th>Auftraggeber</th><th>Wert</th></tr><tr><td>Kantone</td><td>8'700'000&nbsp;CHF</td></tr></table>",
                    vec![],
                )],
            )],
        ));
        let arts = lexwork_articles(&payload, "fr").unwrap();
        assert_eq!(
            arts[0].texte,
            "Auftraggeber | Wert\nKantone | 8'700'000 CHF"
        );
    }

    #[test]
    fn a_wholly_repealed_article_has_left_the_act() {
        let repealed_paragraph = |n: &str| {
            node(
                "paragraph",
                &format!(
                    "<div class='paragraph'><span class='number'>{n}</span><p><span class='abrogation_ellip'>&hellip;&nbsp;<strong>*</strong></span></p></div>"
                ),
                vec![],
            )
        };
        let payload = version(node(
            "title",
            "",
            vec![
                article(
                    "5",
                    "",
                    vec![repealed_paragraph("1"), repealed_paragraph("2")],
                ),
                article(
                    "6",
                    "",
                    vec![
                        repealed_paragraph("1"),
                        node(
                            "paragraph",
                            "<div class='paragraph'><span class='number'>2</span><p>Reste.</p></div>",
                            vec![],
                        ),
                    ],
                ),
            ],
        ));
        let arts = lexwork_articles(&payload, "fr").unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].num, "6");
        assert_eq!(arts[0].texte, "1 …\n2 Reste.");
    }
}
