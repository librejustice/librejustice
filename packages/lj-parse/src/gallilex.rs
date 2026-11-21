//! Gallilex, the French Community's base of its norms: an act's page, and its coordinated PDF.

use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::ConsolidatedArticle;
use crate::pdf_articles::{pdf_articles, Words};

static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\[?)(?i:Article|Art\.)\s*(premier|[0-9]+[A-Za-z]*(?:\s(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|novies|decies|undecies|duodecies|terdecies|quaterdecies|quindecies|sexdecies|septdecies|octodecies|novodecies|vicies)\b)?(?:[./][0-9]+[A-Za-z]*)*)\.?\s*[-\x{2013}:]?\s*").unwrap()
});
static SOLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\[?)(?i:Article\s+unique)\.?\s*[-\x{2013}:]?\s*").unwrap());
static CLOSING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:Promulguons|(?:Donnée?|Fait) à [A-Z]|Bruxelles, le \d)").unwrap()
});
static HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:Docu(?: n°)? \d+|Lois \d+|p\. ?\d+)$").unwrap());
static FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:Centre de documentation administrative|Secrétariat général|Direction des Publications juridiques)").unwrap()
});
static UPDATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:mise? à jour(?: au)?|version initiale au) (\d{2})[/-](\d{2})[/-](\d{4})")
        .unwrap()
});
static TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<h1 class="node-title"><time datetime="(\d{4}-\d{2}-\d{2})[^"]*">[^<]*</time>\s*-\s*(.*?)</h1>"#)
        .unwrap()
});
static DOCUMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<p>Document n° (\d+)</p>").unwrap());
static MONITEUR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)>Date de Moniteur</div>\s*<div[^>]*>\s*<time datetime="(\d{4}-\d{2}-\d{2})"#)
        .unwrap()
});
static PDF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<a href="([^"]+\.pdf)"[^>]*>Consulter le texte"#).unwrap());

fn level(heading: &str) -> Option<u8> {
    let word = heading
        .trim_start_matches(['[', ' '])
        .split([' ', '.'])
        .next()?;
    match word.to_lowercase().as_str() {
        "partie" => Some(1),
        "livre" => Some(2),
        "titre" => Some(3),
        "chapitre" => Some(4),
        "section" => Some(5),
        "sous-section" => Some(6),
        _ => None,
    }
}

static FRENCH: Words = Words {
    head: &HEAD,
    sole: &SOLE,
    closing: &CLOSING,
    annexed: true,
    level,
    furniture: Some((&HEADER, &FOOTER)),
    restart: None,
};

/// An act's page: its number, title, adoption and Moniteur days (`YYYY-MM-DD`), and its PDF's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GallilexPage {
    pub document: String,
    pub title: String,
    pub adopted: String,
    pub published: Option<String>,
    pub pdf: String,
}

pub fn gallilex_page(html: &str) -> Result<GallilexPage, String> {
    let title = TITLE.captures(html).ok_or("no title")?;
    Ok(GallilexPage {
        document: DOCUMENT.captures(html).ok_or("no document number")?[1].to_string(),
        title: html_escape::decode_html_entities(&title[2])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        adopted: title[1].to_string(),
        published: MONITEUR.captures(html).map(|c| c[1].to_string()),
        pdf: PDF.captures(html).ok_or("no pdf")?[1].to_string(),
    })
}

/// A coordinated PDF's articles, and the day the coordination is up to date (`YYYY-MM-DD`).
pub fn gallilex_articles(xml: &str) -> Result<(Vec<ConsolidatedArticle>, Option<String>), String> {
    let updated = UPDATED
        .captures(xml)
        .map(|c| format!("{}-{}-{}", &c[3], &c[2], &c[1]));
    Ok((pdf_articles(xml, &FRENCH)?, updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(top: i32, left: i32, font: u32, inner: &str) -> String {
        format!(
            r#"<text top="{top}" left="{left}" width="300" height="18" font="{font}">{inner}</text>"#
        )
    }

    #[test]
    fn a_coordinated_pdf_reads_its_articles_without_its_running_header_footer_notes_signatures_or_annex(
    ) {
        let page = |n: u32, body: Vec<String>| {
            [
                format!(r#"<page number="{n}" position="absolute" top="0" left="0" height="1262" width="892">"#),
                run(111, 162, 1, "<i>Docu 54065</i> "),
                run(111, 780, 2, &format!("<b>p.{n}</b> ")),
                run(1150, 162, 3, "Direction des Publications juridiques et de la Transparence "),
                run(1147, 667, 0, "A.Gt. 12-06-2026 "),
                run(1168, 162, 3, "Secrétariat général "),
                run(1168, 600, 3, "Version initiale au 07/07/2026"),
            ]
            .into_iter()
            .chain(body)
            .chain(["</page>".to_string()])
            .collect::<Vec<_>>()
            .join("\n")
        };
        let xml = [
            r##"<fontspec id="0" size="18" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            r##"<fontspec id="1" size="18" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            r##"<fontspec id="2" size="18" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            r##"<fontspec id="3" size="15" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            r##"<fontspec id="4" size="21" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            r##"<fontspec id="8" size="12" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            r##"<fontspec id="9" size="16" family="CenturySchoolbook" color="#000000"/>"##
                .to_string(),
            page(
                1,
                vec![
                    run(
                        135,
                        189,
                        4,
                        "<b>Arrêté du Gouvernement de la Communauté française </b>",
                    ),
                    run(160, 213, 4, "<b>dérogeant à l'article 24 </b>"),
                    run(333, 277, 4, "<b>A.Gt. 12-06-2026 </b> "),
                    run(396, 204, 0, "Le Gouvernement de la Communauté française, "),
                    run(432, 204, 0, "Vu le décret du 27 février 2003 ; "),
                    run(
                        450,
                        400,
                        2,
                        "<b>CHAPITRE I</b><b>er</b><b>. - Dispositions </b>",
                    ),
                    run(468, 430, 2, "<b>générales </b>"),
                ],
            ),
            page(
                2,
                vec![
                    run(311, 204, 2, "<b>Article 1</b>"),
                    run(317, 287, 8, "<b>er</b>"),
                    run(311, 297, 2, "<b>. - </b>Par dérogation à l'article 24, "),
                    run(
                        329,
                        162,
                        0,
                        "les centres sont subventionnés<a href=\"54065.html#1\">.]</a> ",
                    ),
                    run(455, 390, 9, "<i><b>Modifié par A.Gt 15-07-2026 </b></i>"),
                    run(
                        473,
                        204,
                        2,
                        "<b>Article 2. - </b>Le présent arrêté produit ses effets. ",
                    ),
                    run(509, 204, 0, "Bruxelles, le 12 juin 2026. "),
                    run(527, 162, 0, "Le Ministre des Sports, "),
                    run(563, 450, 2, "<b>Annexe </b>"),
                    run(
                        599,
                        204,
                        2,
                        "<b>Article 3. - </b>Les centres agréés sont les suivants. ",
                    ),
                ],
            ),
        ]
        .join("\n");
        let (articles, updated) = gallilex_articles(&xml).unwrap();
        let read: Vec<(&str, Option<&str>, &str)> = articles
            .iter()
            .map(|a| (a.num.as_str(), a.title_path.as_deref(), a.texte.as_str()))
            .collect();
        let chapter = Some("CHAPITRE Ier. - Dispositions générales");
        assert_eq!(
            read,
            [
                (
                    "1er",
                    chapter,
                    "Par dérogation à l'article 24,\nles centres sont subventionnés.]"
                ),
                ("2", chapter, "Le présent arrêté produit ses effets."),
            ]
        );
        assert_eq!(updated.as_deref(), Some("2026-07-07"));
    }

    #[test]
    fn a_page_reads_its_number_title_days_and_pdf() {
        let html = r#"<h1 class="node-title"><time datetime="2024-11-04T12:00:00Z">04/11/2024</time>
 - Arrêté ministériel portant sur l'adoption du référentiel &laquo; Garde &raquo;</h1>
<div class="node-header-data"><p>Document n° 52850</p></div>
<a href="/sites/default/files/textes-normatifs/2024-11/20241104s52850%20%282024_0627%29.pdf" class="btn btn-primary" target="_blank">Consulter le texte (PDF)</a>
<div class="row node-data-item">
    <div class="col-lg-3 col-md-4 node-data-label">Date de Moniteur</div>
    <div class="col-lg-9 col-md-8">
        <time datetime="2024-11-29T12:00:00Z">29/11/2024</time>
    </div>
</div>"#;
        assert_eq!(
            gallilex_page(html).unwrap(),
            GallilexPage {
                document: "52850".into(),
                title: "Arrêté ministériel portant sur l'adoption du référentiel « Garde »".into(),
                adopted: "2024-11-04".into(),
                published: Some("2024-11-29".into()),
                pdf: "/sites/default/files/textes-normatifs/2024-11/20241104s52850%20%282024_0627%29.pdf"
                    .into(),
            }
        );
    }
}
