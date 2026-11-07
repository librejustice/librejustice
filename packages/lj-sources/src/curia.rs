//! Curia: its former listing application gives a document's docid per language, its blob service the text.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use tokio::sync::Mutex;

use crate::error::Result;
use crate::html_strip::strip_html;

pub const LISTING_URL: &str = "https://juris.curia.europa.eu/juris/liste.jsf";
pub const ROBOTS_URL: &str = "https://curia.europa.eu/robots.txt";
pub const BLOB_URL: &str = "https://infocuriaws.curia.europa.eu/blob/download-file";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(3);

/// A document of a Curia listing, and the languages it is served in, upper case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuriaDocument {
    pub docid: u64,
    pub html: Vec<String>,
    pub pdf: Vec<String>,
}

/// The docids and case numbers the Court's `robots.txt` withholds from robots.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Withheld {
    pub docids: Vec<u64>,
    pub cases: Vec<String>,
}

pub struct CuriaClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
    withheld: Withheld,
}

impl CuriaClient {
    pub async fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        let robots = http
            .get(ROBOTS_URL)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(Self {
            http,
            last: Mutex::new(None),
            withheld: withheld(&robots),
        })
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
        }
        let response = request.send().await;
        *last = Some(Instant::now());
        Ok(response?)
    }

    /// The document `celex` names in Curia's listing, looked up by its ECLI, else by its case number.
    pub async fn find(&self, celex: &str, ecli: Option<&str>) -> Result<Option<CuriaDocument>> {
        let ecli = ecli.filter(|e| !e.is_empty());
        let mut queries = Vec::new();
        if let Some(e) = ecli {
            queries.push(("critereEcli", e.to_string()));
        }
        let number = lj_parse::celex_to_case_number(celex);
        if self.withheld.cases.contains(&number) {
            return Ok(None);
        }
        if number != celex {
            queries.push(("num", number));
        }
        for (key, value) in queries {
            let request = self
                .http
                .get(LISTING_URL)
                .query(&[(key, value.as_str()), ("language", "fr")]);
            let listing = self.send(request).await?.error_for_status()?.text().await?;
            if let Some(document) = find_document(&listing, celex, ecli) {
                return Ok(Some(document).filter(|d| !self.withheld.docids.contains(&d.docid)));
            }
        }
        Ok(None)
    }

    /// The text of `document` in `lang` (ISO 639-1), off its PDF when no HTML is served.
    pub async fn text(&self, document: &CuriaDocument, lang: &str) -> Result<Option<String>> {
        let lang = lang.to_ascii_uppercase();
        let kind = if document.html.contains(&lang) {
            "html"
        } else if document.pdf.contains(&lang) {
            "pdf"
        } else {
            return Ok(None);
        };
        let url = format!("{BLOB_URL}/{}/{lang}/{kind}", document.docid);
        let response = self.send(self.http.get(&url)).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response.error_for_status()?;
        let text = if kind == "html" {
            strip_html(&response.text().await?)
        } else {
            lj_parse::reflow_page_text(&crate::pdf::pdftotext_extract(&response.bytes().await?)?)
        };
        Ok(Some(text).filter(|t| !t.trim().is_empty()))
    }
}

/// What the `Disallow` lines of `robots` name by `docid=` or `num=`.
pub fn withheld(robots: &str) -> Withheld {
    static DOCID: OnceLock<Regex> = OnceLock::new();
    static NUM: OnceLock<Regex> = OnceLock::new();
    let docid = DOCID.get_or_init(|| Regex::new(r"docid=(\d+)").unwrap());
    let num = NUM.get_or_init(|| Regex::new(r"num=([A-Z]-\d+/\d+)").unwrap());
    let mut out = Withheld::default();
    for rule in robots.split("Disallow:").skip(1) {
        for c in docid.captures_iter(rule) {
            if let Ok(d) = c[1].parse() {
                if !out.docids.contains(&d) {
                    out.docids.push(d);
                }
            }
        }
        for c in num.captures_iter(rule) {
            if !out.cases.iter().any(|n| n == &c[1]) {
                out.cases.push(c[1].to_string());
            }
        }
    }
    out
}

/// The row of `listing` whose ECLI or EUR-Lex link names the CELEX, if it links a text.
pub fn find_document(listing: &str, celex: &str, ecli: Option<&str>) -> Option<CuriaDocument> {
    static HTML: OnceLock<Regex> = OnceLock::new();
    static PDF: OnceLock<Regex> = OnceLock::new();
    let html = HTML.get_or_init(|| {
        Regex::new(r#"document\.jsf[^"]*?docid=(\d+)[^"]*?doclang=([A-Za-z]{2})"#).unwrap()
    });
    let pdf = PDF.get_or_init(|| {
        Regex::new(r#"showPdf\.jsf[^"]*?docid=(\d+)[^"]*?doclang=([A-Za-z]{2})"#).unwrap()
    });
    let listing = listing.replace("&amp;", "&");
    let links: Vec<String> = celex_forms(celex)
        .into_iter()
        .map(|c| format!("CELEX:{c}\""))
        .collect();
    let row = listing
        .split("<tr class=\"table_document_ligne\">")
        .skip(1)
        .map(|row| row.split("</tr>").next().unwrap_or(row))
        .find(|row| {
            ecli.is_some_and(|e| row.contains(&format!(">{e}<")))
                || links.iter().any(|l| row.contains(l.as_str()))
        })?;
    let langs = |re: &Regex| -> (Option<u64>, Vec<String>) {
        let mut docid = None;
        let mut langs: Vec<String> = Vec::new();
        for c in re.captures_iter(row) {
            docid = docid.or_else(|| c[1].parse().ok());
            let lang = c[2].to_ascii_uppercase();
            if !langs.contains(&lang) {
                langs.push(lang);
            }
        }
        (docid, langs)
    };
    let (html_docid, html) = langs(html);
    let (pdf_docid, pdf) = langs(pdf);
    Some(CuriaDocument {
        docid: html_docid.or(pdf_docid)?,
        html,
        pdf,
    })
}

/// The CELEX, and the older form some of Curia's links carry (`61993A0001` for `61993TJ0001`).
fn celex_forms(celex: &str) -> Vec<String> {
    let mut forms = vec![celex.to_string()];
    let old = match celex.get(5..7) {
        Some("TJ") => "A",
        Some("TO") => "B",
        Some("CJ") => "J",
        Some("CO") => "O",
        Some("CC") => "C",
        _ => return forms,
    };
    forms.push(format!("{}{old}{}", &celex[..5], &celex[7..]));
    forms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ecli: &str, celex: &str, docid: u64, langs: &[&str]) -> String {
        let docs: String = langs
            .iter()
            .map(|l| {
                format!(r#"<a href="/juris/document/document.jsf;jsessionid=X?text=&amp;docid={docid}&amp;pageIndex=0&amp;doclang={l}&amp;mode=lst">{l}</a>"#)
            })
            .collect();
        format!(
            r#"<tr class="table_document_ligne"><td class="liste_table_cell_doc">Arrêt <br /><span class="outputEcliAff">{ecli}</span></td><td>{docs}</td><td><a href="https://eur-lex.europa.eu/legal-content/fr/TXT/?uri=CELEX:{celex}" target="_blank"></a></td></tr>"#
        )
    }

    #[test]
    fn the_row_of_the_celex_is_kept_and_not_its_neighbour_in_the_case() {
        let listing = format!(
            r#"<div class="decision">{}{}</div>"#,
            row(
                "ECLI:EU:C:2023:375",
                "62020CC0560",
                273308,
                &["FR", "EN", "DE"]
            ),
            row(
                "ECLI:EU:C:2024:96",
                "62020CJ0560",
                282262,
                &["fr", "FR", "IT"]
            ),
        );
        let judgment = CuriaDocument {
            docid: 282262,
            html: vec!["FR".into(), "IT".into()],
            pdf: vec![],
        };
        assert_eq!(
            find_document(&listing, "62020CJ0560", Some("ECLI:EU:C:2024:96")),
            Some(judgment.clone())
        );
        assert_eq!(find_document(&listing, "62020CJ0560", None), Some(judgment));
        assert_eq!(find_document(&listing, "62021CJ0001", None), None);
    }

    #[test]
    fn what_the_robots_file_disallows_by_docid_or_case_is_withheld() {
        let robots = "User-agent: *\nDisallow: /common/recdoc/index_9_06_02.htm\n\
            Disallow: /juris/document/document.jsf?*docid=163787*\n\
            Disallow: /juris/document/document.jsf?*docid=171241*Disallow: /juris/document/document.jsf?*num=F-50/14*\n\
            Disallow: /juris/liste.jsf?*language=en&num=F-51/14*\n\
            Disallow: /juris/liste.jsf?*num=F-51/14*\n";
        assert_eq!(
            withheld(robots),
            Withheld {
                docids: vec![163787, 171241],
                cases: vec!["F-50/14".into(), "F-51/14".into()],
            }
        );
    }

    #[test]
    fn the_old_form_of_a_celex_is_tried_too() {
        assert_eq!(celex_forms("61993TJ0001"), ["61993TJ0001", "61993A0001"]);
    }
}
