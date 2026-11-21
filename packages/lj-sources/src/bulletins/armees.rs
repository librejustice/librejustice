//! Bulletin officiel des armées: a tree of year and month pages, each month
//! listing its weekly issues, each text with its own PDF.

use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::{french_long_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::Result;
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.defense.gouv.fr";
const ROOT: &str = "/bulletin-officiel/textes-officiels-du-ministere-armees";

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [year_link, month_link] = RE.get_or_init(|| {
        [
            Regex::new(r#"href="(/bulletin-officiel/textes-officiels-du-ministere-armees/textes-officiels-du-ministere-armees-[^"/]*?(\d{4}))""#)
                .expect("valid regex"),
            Regex::new(r#"href="(/bulletin-officiel/textes-officiels-du-ministere-armees/textes-officiels-du-ministere-armees-[^"/]+/boc[^"/]*)""#)
                .expect("valid regex"),
        ]
    });
    let root = client.get_text(&format!("{BASE_URL}{ROOT}")).await?;
    let mut years: Vec<String> = year_link
        .captures_iter(&root)
        .filter(|c| c[2].parse::<i32>().is_ok_and(|y| y >= since_year))
        .map(|c| c[1].to_string())
        .collect();
    years.sort();
    years.dedup();
    let mut out = Vec::new();
    for year in years {
        let page = client.get_text(&format!("{BASE_URL}{year}")).await?;
        let mut months: Vec<String> = month_link
            .captures_iter(&page)
            .map(|c| c[1].to_string())
            .collect();
        months.sort();
        months.dedup();
        for month in months {
            out.extend(parse_month(
                &client.get_text(&format!("{BASE_URL}{month}")).await?,
            ));
        }
    }
    keep_reused_files_for_the_latest(&mut out);
    Ok(out)
}

/// The site uploads a later text under an earlier text's file name, and the URL serves the later one.
fn keep_reused_files_for_the_latest(notices: &mut [BulletinNotice]) {
    let mut latest = std::collections::HashMap::<String, Option<NaiveDate>>::new();
    for n in notices.iter() {
        if let Some(url) = &n.body_url {
            let d = latest.entry(url.clone()).or_insert(n.published);
            *d = (*d).max(n.published);
        }
    }
    for n in notices.iter_mut() {
        if n.body_url
            .as_ref()
            .is_some_and(|u| latest[u] != n.published)
        {
            n.body_url = None;
            n.notice_url = format!("{BASE_URL}{ROOT}");
        }
    }
}

#[derive(Default)]
struct Block {
    items: Vec<(String, Option<String>, Option<NaiveDate>)>,
    pdfs: Vec<(String, String)>,
}

/// Recent issues put each text in a block of its own followed by its PDF;
/// older ones list several texts, then their PDFs in the same order.
pub fn parse_month(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [token, tags, issue_number, numbered] = RE.get_or_init(|| {
        [
            Regex::new(concat!(
                r#"(?s)<h2([^>]*)>(.*?)</h2>"#,
                r#"|<li>(.*?)</li>|<p[^>]*>(.*?)</p>"#,
                r#"|<a\s+href="([^"]+)"[^>]*?title="([^"]*)"[^>]*class="fr-download__link"#,
                r#"|(<section class="fr-container fr-download-file_container">)"#,
                r#"|(<div class="fr-richtext fr-container">)"#,
            ))
            .expect("valid regex"),
            Regex::new(r"<[^>]+>").expect("valid regex"),
            Regex::new(r"N°\s*(\d+)").expect("valid regex"),
            Regex::new(r"(?i)fichier\s+(?:texte\s*)?(\d+)\b").expect("valid regex"),
        ]
    });
    let start = html.find("content-container__paragraph").unwrap_or(0);
    let end = html[start..]
        .find("Partager la page")
        .map_or(html.len(), |e| start + e);
    let clean = |s: &str| -> String {
        decode_entities(&tags.replace_all(s, " "))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };

    let (mut issue, mut published) = (None::<String>, None::<NaiveDate>);
    let mut blocks: Vec<Block> = Vec::new();
    let mut in_pdfs = false;
    for t in token.captures_iter(&html[start..end]) {
        if let (Some(attrs), Some(heading)) = (t.get(1), t.get(2)) {
            let heading = clean(heading.as_str());
            if !attrs.as_str().contains("fr-download-file__title") && !heading.is_empty() {
                published = french_long_date(&heading);
                issue = Some(match (issue_number.captures(&heading), published) {
                    (Some(n), Some(d)) => format!("{}-{}", d.format("%Y"), &n[1]),
                    (_, Some(d)) => d.format("%Y%m%d").to_string(),
                    _ => heading,
                });
            }
        } else if t.get(8).is_some() {
            if in_pdfs || blocks.is_empty() {
                blocks.push(Block::default());
            }
            in_pdfs = false;
        } else if t.get(7).is_some() {
            in_pdfs = true;
        } else if let (Some(href), Some(label)) = (t.get(5), t.get(6)) {
            if blocks.is_empty() {
                blocks.push(Block::default());
            }
            let href = href.as_str().replacen("http://", "https://", 1);
            blocks
                .last_mut()
                .expect("a block")
                .pdfs
                .push((href, decode_entities(label.as_str())));
        } else {
            let text = clean(t.get(3).or(t.get(4)).map_or("", |m| m.as_str()));
            if text.chars().count() < 15 {
                continue;
            }
            if in_pdfs || blocks.is_empty() {
                blocks.push(Block::default());
                in_pdfs = false;
            }
            blocks
                .last_mut()
                .expect("a block")
                .items
                .push((text, issue.clone(), published));
        }
    }

    let mut position = std::collections::HashMap::<String, usize>::new();
    let mut out = Vec::new();
    for b in blocks {
        let numbers: Vec<Option<usize>> = b
            .pdfs
            .iter()
            .map(|(_, label)| numbered.captures(label).and_then(|c| c[1].parse().ok()))
            .collect();
        for ((title, issue, published), pdf) in b.items.iter().zip(pair(&b, &numbers)) {
            let Some(issue) = issue else { continue };
            let n = position.entry(issue.clone()).or_default();
            *n += 1;
            let kind = kind(title);
            out.push(BulletinNotice {
                site: BulletinSite::Armees,
                native_id: format!("{issue}-{n}"),
                nor: None,
                signed: french_long_date(title),
                published: *published,
                notice_url: pdf
                    .map_or_else(|| format!("{BASE_URL}{ROOT}"), |(href, _)| href.clone()),
                body_url: pdf.map(|(href, _)| href.clone()),
                body: None,
                legifrance_uid: None,
                abrogated: false,
                doctrine: matches!(
                    kind.as_str(),
                    "Instruction" | "Circulaire" | "Directive" | "Note"
                ),
                kind,
                title: title.clone(),
            });
        }
    }
    out
}

/// By rank when the counts match, else by the label's number, the rest by unnumbered PDF.
fn pair<'a>(b: &'a Block, numbers: &[Option<usize>]) -> Vec<Option<&'a (String, String)>> {
    if b.items.len() == 1 || b.items.len() == b.pdfs.len() {
        return (0..b.items.len()).map(|k| b.pdfs.get(k)).collect();
    }
    let mut out: Vec<_> = (1..=b.items.len())
        .map(|k| {
            b.pdfs
                .iter()
                .zip(numbers)
                .find(|(_, n)| **n == Some(k))
                .map(|(p, _)| p)
        })
        .collect();
    let left: Vec<usize> = (0..out.len()).filter(|&k| out[k].is_none()).collect();
    let loose: Vec<&(String, String)> = b
        .pdfs
        .iter()
        .zip(numbers)
        .filter(|(_, n)| n.is_none())
        .map(|(p, _)| p)
        .collect();
    if left.len() == 1 || left.len() == loose.len() {
        for (k, p) in left.into_iter().zip(loose) {
            out[k] = Some(p);
        }
    }
    out
}

/// The title's first word, after the rank some issues number their texts with.
fn kind(title: &str) -> String {
    let word = title
        .split_whitespace()
        .find(|w| !w.chars().all(|c| c.is_ascii_digit() || ".)-".contains(c)))
        .unwrap_or("");
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_listed_together_take_their_pdfs_in_order() {
        let html = r#"content-container__paragraph">
          <div class="fr-richtext fr-container">
            <h2 data-summary-enabled="1" id="title-84852">BOC N° 28 du 6 avril 2020</h2>
            <ol>
              <li>Instruction N° 87/ARM/DGA du 11 mars 2020 relative aux missions (Abrogé par : Instruction N° 87 du 15 mai 2025)</li>
              <li>Erratum à la décision N° 630/ARM/DCSCA du 12 mars 2020</li>
            </ol>
          </div>
          <section class="fr-container fr-download-file_container">
            <h2 class="fr-download-file__title"></h2>
            <a href="http://www.defense.gouv.fr/sites/default/files/sga/1%20INSTRUCTION.pdf" target="_blank" title="Visualiser et télécharger le fichier 1 INSTRUCTION N° 87" class="fr-download__link fr-link">x</a>
            <a href="http://www.defense.gouv.fr/sites/default/files/sga/2%20ERRATUM.pdf" target="_blank" title="Visualiser et télécharger le fichier 2 ERRATUM" class="fr-download__link fr-link">x</a>
          </section>
          <div class="fr-richtext fr-container">
            <h2 data-summary-enabled="1" id="title-101481">BOC N° 93 du 5 décembre 2025</h2>
            <ul><li>Circulaire N° 855/ARM/DSEO/SDRH du 01 décembre 2025 relative au concours de recrutement.</li></ul>
          </div>
          <section class="fr-container fr-download-file_container">
            <a href="http://www.defense.gouv.fr/sites/default/files/bulletin-officiel/Circulaire%20N%C2%B0%20855.pdf" target="_blank" title="Visualiser et télécharger le fichier Circulaire N° 855" class="fr-download__link fr-link">x</a>
          </section>
          Partager la page"#;
        let notices = parse_month(html);
        assert_eq!(notices.len(), 3);
        let n = &notices[0];
        assert_eq!(n.text_uid(), "bo_armees_2020-28-1");
        assert_eq!(n.kind, "Instruction");
        assert!(n.doctrine);
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2020, 3, 11));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2020, 4, 6));
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://www.defense.gouv.fr/sites/default/files/sga/1%20INSTRUCTION.pdf")
        );
        assert_eq!(notices[1].kind, "Erratum");
        assert!(!notices[1].doctrine);
        assert!(notices[1]
            .body_url
            .as_deref()
            .is_some_and(|u| u.ends_with("2%20ERRATUM.pdf")));
        let n = &notices[2];
        assert_eq!(n.text_uid(), "bo_armees_2025-93-1");
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2025, 12, 5));
        assert!(n
            .body_url
            .as_deref()
            .is_some_and(|u| u.ends_with("Circulaire%20N%C2%B0%20855.pdf")));
    }

    #[test]
    fn a_file_reused_by_a_later_issue_is_the_later_text() {
        let html = r#"content-container__paragraph">
          <div class="fr-richtext fr-container">
            <h2 data-summary-enabled="1" id="title-1">BOC N° 60 du 7 août 2020</h2>
            <ul><li>Instruction N° 520/ARM/DGA/DO/SDAQ du 26 juin 2020 relative aux missions</li></ul>
          </div>
          <section class="fr-container fr-download-file_container">
            <a href="https://www.defense.gouv.fr/sga/1%20INSTRUCTION%20520.pdf" title="Visualiser et télécharger le fichier 1 INSTRUCTION N° 520" class="fr-download__link">x</a>
          </section>
          <div class="fr-richtext fr-container">
            <h2 data-summary-enabled="1" id="title-2">BOC N° 49 du 1er juillet 2022</h2>
            <ul><li>Instruction N° 520/ARM/DGA/DO/SDAQ du 22 juin 2022 relative aux missions</li></ul>
          </div>
          <section class="fr-container fr-download-file_container">
            <a href="https://www.defense.gouv.fr/sga/1%20INSTRUCTION%20520.pdf" title="Visualiser et télécharger le fichier 1 INSTRUCTION N° 520" class="fr-download__link">x</a>
          </section>
          Partager la page"#;
        let mut notices = parse_month(html);
        keep_reused_files_for_the_latest(&mut notices);
        assert_eq!(notices[0].body_url, None);
        assert_eq!(
            notices[1].body_url.as_deref(),
            Some("https://www.defense.gouv.fr/sga/1%20INSTRUCTION%20520.pdf")
        );
    }

    #[test]
    fn a_text_without_a_numbered_pdf_takes_the_unnumbered_one() {
        let html = r#"content-container__paragraph">
          <div class="fr-richtext fr-container">
            <h2 data-summary-enabled="1" id="title-1">BOC n° 108 du 2 juillet 2019</h2>
            <ol>
              <li>Instruction N° 1501/ARM/DMAé du 04 juin 2019 relative au contrôle interne</li>
              <li>Instruction N° 677/ARM/RH-AT du 25 avril 2019 relative au recrutement</li>
              <li>Décision n° 1185/ARM/EMM du 19 juin 2019 portant création de la formation</li>
            </ol>
          </div>
          <section class="fr-container fr-download-file_container">
            <a href="https://www.defense.gouv.fr/a/1501-v1.pdf" title="Visualiser et télécharger le fichier INSTRUCTION N° 1501 V1" class="fr-download__link">x</a>
            <a href="https://www.defense.gouv.fr/a/1501-v2.pdf" title="Visualiser et télécharger le fichier INSTRUCTION N° 1501 V2" class="fr-download__link">x</a>
            <a href="https://www.defense.gouv.fr/a/677.pdf" title="Visualiser et télécharger le fichier 2 INSTRUCTION N° 677" class="fr-download__link">x</a>
            <a href="https://www.defense.gouv.fr/a/1185.pdf" title="Visualiser et télécharger le fichier 3 DÉCISION n° 1185" class="fr-download__link">x</a>
          </section>
          Partager la page"#;
        let urls: Vec<_> = parse_month(html)
            .into_iter()
            .map(|n| n.body_url.unwrap_or_default())
            .collect();
        assert_eq!(
            urls,
            [
                "https://www.defense.gouv.fr/a/1501-v1.pdf",
                "https://www.defense.gouv.fr/a/677.pdf",
                "https://www.defense.gouv.fr/a/1185.pdf",
            ]
        );
    }
}
