//! Madagascar's legal texts as CNLEGIS files them: type, number, day, Journal officiel, status
//! and notes, with the French content in HTML or, for the recent ones, a link to its PDF.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// A text as a page of a search answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CnlegisText {
    pub id: u64,
    /// The type CNLEGIS files it under, `Loi`, `Décret`, `Arrêté`.
    pub kind: String,
    pub number: String,
    /// The text's own day, `YYYY-MM-DD`.
    pub day: Option<String>,
    pub object: String,
    /// `En vigueur`, `Abrogé` or `Périmé`.
    pub status: String,
    /// The notes CNLEGIS keeps on the text: its amendments, repeals and implementing acts.
    pub notes: Vec<String>,
    pub ministry: Option<String>,
    /// The Journal officiel's number, `None` where CNLEGIS writes a placeholder.
    pub issue: Option<String>,
    pub issue_day: Option<String>,
    pub issue_page: Option<i32>,
    pub html: String,
    /// The PDF the content links to when it holds no more than a header.
    pub pdf: Option<String>,
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</div>|</li>").unwrap());
static PDF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://cnlegis\.gov\.mg/uploads/[^"'\s<>]+\.pdf"#).unwrap());

fn plain(html: &str) -> String {
    let text = TAG.replace_all(html, " ");
    html_escape::decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `dd-mm-yyyy` as `yyyy-mm-dd`, `None` for a day that is not one.
fn iso(day: Option<&str>) -> Option<String> {
    let mut parts = day?.trim().split('-');
    let (d, m, y) = (parts.next()?, parts.next()?, parts.next()?);
    let date = jiff::civil::Date::new(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?).ok()?;
    Some(date.to_string())
}

fn field<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn kept(s: Option<&str>) -> Option<String> {
    s.map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

/// A page of a search: the type the session searches, empty once the session is lost, and its
/// texts, none past the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CnlegisPage {
    pub searched: String,
    pub texts: Vec<CnlegisText>,
}

pub fn cnlegis_page(json: &str) -> Result<CnlegisPage, String> {
    if json.trim().is_empty() {
        return Ok(CnlegisPage {
            searched: String::new(),
            texts: Vec::new(),
        });
    }
    let page: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let rows = page
        .get("aaData")
        .and_then(Value::as_array)
        .ok_or("a page without aaData")?;
    let texts = rows
        .iter()
        .map(|r| {
            let id = field(r, "id")
                .and_then(|id| id.trim().parse().ok())
                .ok_or_else(|| format!("a text without an id: {:?}", r.get("id")))?;
            let html = field(r, "html_fichier_fr").unwrap_or_default().to_string();
            let notes = field(r, "notes")
                .map(|n| {
                    BREAK
                        .replace_all(n, "\n")
                        .lines()
                        .map(plain)
                        .map(|l| l.trim_start_matches(['.', '-', ' ']).to_string())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Ok(CnlegisText {
                id,
                kind: kept(field(r, "type_txt")).unwrap_or_default(),
                number: kept(field(r, "num_txt"))
                    .map(|n| n.trim_end_matches(['\'', '.']).to_string())
                    .unwrap_or_default(),
                day: iso(field(r, "date_txt")),
                object: field(r, "objet_txt").map(plain).unwrap_or_default(),
                status: kept(field(r, "etat_txt")).unwrap_or_default(),
                notes,
                ministry: kept(field(r, "ministere")),
                issue: kept(field(r, "num_jo"))
                    .filter(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit())),
                issue_day: iso(field(r, "date_jo")),
                issue_page: field(r, "page_jo")
                    .and_then(|p| p.trim().parse().ok())
                    .filter(|&p| p > 0),
                pdf: PDF.find(&html).map(|m| m.as_str().to_string()),
                html,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(CnlegisPage {
        searched: field(&page, "selected_type")
            .unwrap_or_default()
            .to_string(),
        texts,
    })
}

/// What a CNLEGIS type is as a norm: the head of its uid and its nature. A court's decision, an
/// explanatory memorandum, a list of laureates or minutes are not norms.
pub fn cnlegis_nature(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "Constitution" => ("CONSTITUTION", "CONSTITUTION"),
        "Loi constitutionnelle" => ("LOI", "LOI_CONSTITUTIONNELLE"),
        "Loi organique" => ("LOI", "LOI_ORGANIQUE"),
        "Loi" => ("LOI", "LOI"),
        "Ordonnance" => ("ORDONNANCE", "ORDONNANCE"),
        "Décret" => ("DECRET", "DECRET"),
        "Arrêté" => ("ARRETE", "ARRETE"),
        "Circulaire" => ("CIRCULAIRE", "CIRCULAIRE"),
        "Décision" => ("DECISION", "DECISION"),
        "Instruction" => ("INSTRUCTION", "INSTRUCTION"),
        "Note" => ("NOTE", "NOTE"),
        "Délibération" => ("DELIBERATION", "DELIBERATION"),
        "Déclaration" => ("DECLARATION", "DECLARATION"),
        _ => return None,
    })
}

/// The High Constitutional Court's decisions, opinions and deliberations, which CNLEGIS files
/// among the texts: `13-HCC/D3-27-07-2026`.
pub fn cnlegis_court(number: &str) -> bool {
    number.to_uppercase().contains("HCC")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reads_each_text_with_its_journal_its_notes_and_its_pdf() {
        let json = r#"{"aaData":[{"date_txt":"05-10-2026","type_txt":"Loi","num_txt":"2026-012","int_name":"L2026-012",
            "objet_txt":"<p><span>Modifiant certaines dispositions de la Loi n&deg; 96-029</span></p>","objet_txt_mg":"",
            "num_jo":"VOIR_J.O","date_jo":"05-10-2026","page_jo":"0","etat_txt":"En vigueur",
            "notes":"<ul><li><p>Abrogée par loi n°2018-011 du 11/07/2018, J.O n°3834</p></li><li>. Voir décret d'application n°2013-082</li></ul>",
            "notes_mg":"","version_pdf_fr":null,"version_pdf_mg":null,"ministere":"PRESIDENCE","id":"58379",
            "html_fichier_fr":"<p><strong>LOI N° 2026-012</strong></p><p><a href=\"https://cnlegis.gov.mg/uploads/L2026-012-VF-.pdf\">Cliquez-ici</a></p>","html_fichier_mg":""},
            {"date_txt":"18-06-2002","type_txt":"Arrêté","num_txt":"609/2002","num_jo":"2781","date_jo":"01-07-2002","page_jo":"1275",
            "etat_txt":"Abrogé","notes":"","ministere":"","id":"36459","html_fichier_fr":"<p>Article premier.- Un.</p>"}],"selected_type":"Loi"}"#;
        let page = cnlegis_page(json).unwrap();
        assert_eq!(page.searched, "Loi");
        let texts = page.texts;
        assert_eq!(texts.len(), 2);
        let law = &texts[0];
        assert_eq!(law.id, 58379);
        assert_eq!(law.day.as_deref(), Some("2026-10-05"));
        assert_eq!(
            law.object,
            "Modifiant certaines dispositions de la Loi n° 96-029"
        );
        assert_eq!(law.issue, None);
        assert_eq!(law.issue_page, None);
        assert_eq!(
            law.notes,
            [
                "Abrogée par loi n°2018-011 du 11/07/2018, J.O n°3834",
                "Voir décret d'application n°2013-082"
            ]
        );
        assert_eq!(
            law.pdf.as_deref(),
            Some("https://cnlegis.gov.mg/uploads/L2026-012-VF-.pdf")
        );
        let arrete = &texts[1];
        assert_eq!(arrete.issue.as_deref(), Some("2781"));
        assert_eq!(arrete.issue_day.as_deref(), Some("2002-07-01"));
        assert_eq!(arrete.issue_page, Some(1275));
        assert_eq!(arrete.ministry, None);
        assert_eq!(arrete.pdf, None);
        assert_eq!(cnlegis_page("").unwrap().searched, "");
    }

    #[test]
    fn a_court_number_is_told_apart() {
        assert!(cnlegis_court("13-HCC/D3-27-07-2026"));
        assert!(!cnlegis_court("063-CENI/D/2024"));
    }
}
