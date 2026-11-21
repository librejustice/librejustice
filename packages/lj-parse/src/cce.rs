//! An arrêt of the Belgian Conseil du contentieux des étrangers (`rvv-cce.be`) into a
//! [`Decision`]: the fields its listing gives, the text of its PDF without its footers.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use lj_core::decision::Decision;

use crate::juportal::BELGIAN_PLACE;
use crate::reflow_page_text;

pub const CCE_CODE: &str = "be_tribunal_administratif_contentieux_etrangers";
pub const CCE_TYPE: &str = "TRIBUNAL_ADMINISTRATIF";
pub const CCE_LABEL: &str = "Conseil du contentieux des étrangers (Belgique)";
pub const CCE_PLACE: &str = BELGIAN_PLACE;

/// The court's Dutch name, as its Dutch rulings print it.
pub fn cce_names() -> Vec<(&'static str, Value)> {
    vec![(
        CCE_CODE,
        json!({ "nl": { "name": "Raad voor Vreemdelingenbetwistingen (België)" } }),
    )]
}

/// A language the court rules in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CceLanguage {
    French,
    Dutch,
    German,
}

/// The languages in the order the stock reads them.
pub const CCE_LANGUAGES: [CceLanguage; 3] =
    [CceLanguage::French, CceLanguage::Dutch, CceLanguage::German];

impl CceLanguage {
    /// The Publications Office code, `fra`.
    pub fn code(self) -> &'static str {
        match self {
            CceLanguage::French => "fra",
            CceLanguage::Dutch => "nld",
            CceLanguage::German => "deu",
        }
    }
}

/// An arrêt as a row of the site's listing gives it, with the language it was listed under.
#[derive(Debug, Clone, Copy)]
pub struct CceArret<'a> {
    pub number: u32,
    pub language: CceLanguage,
    /// `YYYY-MM-DD`.
    pub date: &'a str,
    pub url: &'a str,
    /// `Annulation` or `Plein contentieux`.
    pub procedure: Option<&'a str>,
    /// The applicant's country of origin.
    pub country: Option<&'a str>,
    /// `1 juge`, `3 juges`, `Assemblée Générale`, `Chambres Réunies`.
    pub composition: Option<&'a str>,
    pub judge: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CceRefused {
    /// The PDF holds no text layer worth the name: a scan.
    NoText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CceParsed {
    pub decision: Decision,
    pub source_fields: Value,
}

/// The non-blank characters an arrêt holds at least.
const DECISION_FLOOR: usize = 200;

/// The footer of each page: `CCE X - Page 2`, `CCE n° X / Page 1 sur 5`, `CCE n/ Page 1 sur 7`,
/// `X - Page 1`, `RvV X - Pagina 1 van 3`, `RAS X - Seite 1 von 12`, an en dash now and then.
static FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:CCE|RvV|RAS)?\s*(?:n[°/]\s*)?[X\d\s]*[-\x{2013}/]?\s*(?:Page|Pagina|Seite)\s+\d+(?:\s+(?:sur|van|von)\s+\d+)?$",
    )
    .expect("cce footer regex")
});

/// A footer `pdftotext` glued to the end of a line of text, `de VreemdelingenX - Pagina 1 van 3`.
static GLUED_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"X\s*[-\x{2013}]\s*(?:Page|Pagina|Seite)\s+\d+(?:\s+(?:sur|van|von)\s+\d+)?$")
        .expect("cce glued footer regex")
});

/// `LE PRÉSIDENT F.F. DE LA IIIe CHAMBRE`, `DE LA IE CHAMBRE`, `DE LA Ière CHAMBRE`,
/// `DE LA Xème CHAMBRE`, `DE LA VIIième CHAMBRE`.
static CHAMBRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bDE LA ((?-i:[IVXL]+))(?:i?[eè]me|[eè]re|e)\s+CHAMBRE\b")
        .expect("cce chambre regex")
});

/// `DE WND. VOORZITTER VAN DE IVe KAMER`.
static KAMER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bVAN DE ((?-i:[IVXL]+))(?:de|e)\s+KAMER\b").expect("cce kamer regex")
});

/// `DER DIENSTHABENDE PRÄSIDENT DER VIII. KAMMER`.
static KAMMER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bDER ((?-i:[IVXL]+))\.\s*KAMMER\b").expect("cce kammer regex")
});

pub fn cce_source_uid(number: u32) -> String {
    format!("cce/{number}")
}

/// The chamber the heading names, in the arrêt's language: `IIIe chambre`, `IVe kamer`,
/// `VIII. Kammer`.
fn chamber(language: CceLanguage, text: &str) -> Option<String> {
    let (re, form): (&Regex, fn(&str) -> String) = match language {
        CceLanguage::French => (&CHAMBRE, |r| format!("{r}e chambre")),
        CceLanguage::Dutch => (&KAMER, |r| format!("{r}e kamer")),
        CceLanguage::German => (&KAMMER, |r| format!("{r}. Kammer")),
    };
    re.captures(text).map(|c| form(&c[1]))
}

fn decision_of(number: u32, date: &str, chamber: Option<String>, text: String) -> Decision {
    Decision {
        source_uid: cce_source_uid(number),
        member_name: number.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(CCE_CODE.to_string()),
        chamber,
        nac: None,
        jurisdiction_name: Some(CCE_LABEL.to_string()),
        jurisdiction_type: Some(CCE_TYPE.to_string()),
        jurisdiction_location: None,
        numero_dossier: Some(number.to_string()),
        numero_dossiers: Some(vec![number.to_string()]),
        numero_role: None,
        date_lecture: Some(date.to_string()),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

/// The text `pdftotext` reads off an arrêt, without its page footers, one paragraph a line.
fn clean_text(pdf_text: &str) -> String {
    let bare: Vec<String> = pdf_text
        .lines()
        .filter(|l| !FOOTER.is_match(l.trim()))
        .map(|l| GLUED_FOOTER.replace(l.trim_end(), "").into_owned())
        .collect();
    reflow_page_text(&bare.join("\n"))
}

/// Parses an arrêt from its listing row and the text of its PDF.
pub fn parse_cce(arret: &CceArret<'_>, pdf_text: &str) -> Result<CceParsed, CceRefused> {
    let text = clean_text(pdf_text);
    if text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(CceRefused::NoText);
    }
    let chamber = chamber(arret.language, &text);
    let source_fields = json!({
        "number": arret.number,
        "date": arret.date,
        "chamber": chamber,
        "procedure": arret.procedure,
        "country": arret.country,
        "composition": arret.composition,
        "judge": arret.judge,
        "url_site": arret.url,
        "lang": arret.language.code(),
    });
    Ok(CceParsed {
        decision: decision_of(arret.number, arret.date, chamber, text),
        source_fields,
    })
}

/// Branch of [`Decision::from_source_fields`] for `cce/`, on what the parse stored.
pub(crate) fn from_source_fields_cce(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let number = source_fields
        .get("number")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or_else(|| panic!("source_fields cce without a number ({source_uid})"));
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let date =
        field("date").unwrap_or_else(|| panic!("source_fields cce without a date ({source_uid})"));
    decision_of(
        number,
        date,
        field("chamber").map(str::to_string),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARRET: CceArret<'static> = CceArret {
        number: 159_368,
        language: CceLanguage::French,
        date: "2015-12-24",
        url: "https://www.rvv-cce.be/sites/default/files/arr/A159368.AN.pdf",
        procedure: Some("Annulation"),
        country: Some("Congo (RD)"),
        composition: Some("1 juge"),
        judge: Some("MAERTENS E."),
    };

    const PDF_TEXT: &str = "n° 159 368 du 24 décembre 2015
dans l’affaire X / III
En cause : X
Ayant élu domicile : X
Contre :
l'Etat belge, représenté par la Secrétaire d’Etat à la Politique de migration et
d’asile.
LE PRÉSIDENT DE LA IIIe CHAMBRE,
Vu la requête introduite le 21 mai 2012, par X, qui déclare être de nationalité congolaise, tendant à la
suspension et à l’annulation de la décision de rejet de la demande d’autorisation de séjour.

CCE X \u{2013} Page 1 sur 8
\u{c}Vu le titre Ier bis, chapitre 2, section IV, sous-section 2, de la loi du 15 décembre 1980 sur l’accès au
territoire, le séjour, l’établissement et l’éloignement des étrangers.
CCE X\u{2013} Page 2 sur 8
\u{c}Article unique
La requête en suspension et annulation est rejetée.
CCE X\u{2013} Page 8 sur 8
";

    #[test]
    fn an_arret_loses_its_page_footers_and_reads_its_chamber() {
        let parsed = parse_cce(&ARRET, PDF_TEXT).unwrap();
        let d = &parsed.decision;
        assert_eq!(d.source_uid, "cce/159368");
        assert_eq!(d.chamber.as_deref(), Some("IIIe chambre"));
        assert_eq!(d.numero_dossiers, Some(vec!["159368".to_string()]));
        assert_eq!(d.date_lecture.as_deref(), Some("2015-12-24"));
        assert!(!d.texte_integral_clean.contains("Page"));
        assert!(d
            .texte_integral_clean
            .contains("du 15 décembre 1980 sur l’accès au territoire, le séjour"));
        let restored = from_source_fields_cce(
            &d.texte_integral_clean,
            &parsed.source_fields,
            &d.source_uid,
        );
        assert_eq!(&restored, d);
    }

    #[test]
    fn every_footer_the_court_wrote_is_one() {
        for footer in [
            "CCE n/ Page 1 sur 7",
            "CCE n° X / Page 1 sur 5",
            "CCE X - Page 1",
            "CCE X\u{2013} Page 2 sur 8",
            "X - Page 6",
            "CCE 159 368 - Page 3",
            "RvV X - Pagina 1",
            "X - Pagina 2 van 3",
            "RAS X - Seite 1 von 12",
        ] {
            assert!(FOOTER.is_match(footer), "{footer}");
        }
        for line in [
            "Y./Russie, § 81 ; Cour CCE 364 400 - Page 24 EDH 20 mars 1991",
            "Page 3 du dossier administratif, la partie requérante déclare",
        ] {
            assert!(!FOOTER.is_match(line), "{line}");
        }
    }

    #[test]
    fn every_way_the_court_writes_a_chamber_reads_its_number() {
        for (language, heading, chamber_read) in [
            (
                CceLanguage::French,
                "LA PRÉSIDENTE F.F. DE LA IE CHAMBRE,",
                "Ie chambre",
            ),
            (
                CceLanguage::French,
                "LE PRÉSIDENT DE LA Ière CHAMBRE,",
                "Ie chambre",
            ),
            (
                CceLanguage::French,
                "LA PRESIDENTE DE LA IIIème CHAMBRE,",
                "IIIe chambre",
            ),
            (
                CceLanguage::French,
                "LE PRÉSIDENT F.F. DE LA VIIième CHAMBRE,",
                "VIIe chambre",
            ),
            (
                CceLanguage::French,
                "LE PRÉSIDENT F.F. DE LA Xe CHAMBRE,",
                "Xe chambre",
            ),
            (
                CceLanguage::Dutch,
                "DE WND. VOORZITTER VAN DE IVe KAMER,",
                "IVe kamer",
            ),
            (
                CceLanguage::German,
                "DER DIENSTHABENDE PRÄSIDENT DER VIII. KAMMER,",
                "VIII. Kammer",
            ),
        ] {
            assert_eq!(
                chamber(language, heading).as_deref(),
                Some(chamber_read),
                "{heading}"
            );
        }
        assert_eq!(
            chamber(
                CceLanguage::French,
                "LE CONSEIL DU CONTENTIEUX DES ETRANGERS,"
            ),
            None
        );
    }

    #[test]
    fn a_dutch_arret_loses_a_footer_glued_to_its_text() {
        let arret = CceArret {
            number: 246_587,
            language: CceLanguage::Dutch,
            ..ARRET
        };
        let text = "nr. 246 587 van 21 december 2020
in de zaak RvV X / IX
DE WND. VOORZITTER VAN DE IXe KAMER,
Gezien het verzoekschrift dat X, die verklaart van Afghaanse nationaliteit te zijn, heeft ingediend
tegen de beslissing van de commissaris-generaal. Met toepassing van artikel 39/73 van de wet van
15 december 1980 betreffende de toegang tot het grondgebied, het verblijf, de vestiging en de
verwijdering van vreemdelingen (hierna: de VreemdelingenX - Pagina 1 van 3
\u{c}wet) werd aan de partijen de grond meegedeeld waarop de voorzitter zich steunt om te oordelen dat
het beroep kennelijk ongegrond is. Het beroep wordt verworpen.
X - Pagina 2 van 3
";
        let parsed = parse_cce(&arret, text).unwrap();
        assert_eq!(parsed.source_fields["lang"], "nld");
        assert_eq!(parsed.decision.chamber.as_deref(), Some("IXe kamer"));
        assert!(!parsed.decision.texte_integral_clean.contains("Pagina"));
    }

    #[test]
    fn a_scan_is_refused() {
        assert_eq!(
            parse_cce(&ARRET, "CCE X - Page 1\n\u{c}CCE X - Page 2\n"),
            Err(CceRefused::NoText)
        );
    }
}
