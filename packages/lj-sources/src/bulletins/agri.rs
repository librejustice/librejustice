//! BO-Agri, the agriculture ministry's instructions, listed by number search.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::Datelike;
use regex::Regex;

use super::{french_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://info.agriculture.gouv.fr/boagri";

/// A whole year answers 500 since 2024, so each year is asked by number
/// prefix, `2025-1` holding 2025-1, 2025-10 to 2025-19, 2025-100 to 2025-199…
pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let mut found = BTreeMap::new();
    for year in since_year..=chrono::Utc::now().year() {
        let mut prefixes: Vec<String> = (1..=9).map(|d| format!("{year}-{d}")).collect();
        while let Some(prefix) = prefixes.pop() {
            let url = format!("{BASE_URL}/recherche/resultat_recherche_numero?numero={prefix}");
            match search(client, &url).await? {
                Some(notices) => {
                    for n in notices {
                        found.insert(n.native_id.clone(), n);
                    }
                }
                None if prefix.len() < year.to_string().len() + 4 => {
                    prefixes.extend((0..=9).map(|d| format!("{prefix}{d}")));
                }
                None => return Err(SourceError::Invalid(format!("boagri: {url} keeps failing"))),
            }
        }
    }
    Ok(found.into_values().collect())
}

/// `None` when the server fails twice on the query.
async fn search(client: &BulletinClient, url: &str) -> Result<Option<Vec<BulletinNotice>>> {
    for _ in 0..2 {
        let (status, html) = client.get_page(url).await?;
        if status == 200 && html.contains("liste-resultat") {
            return Ok(Some(parse_listing(&html)));
        }
        if status == 200 && !html.contains("Erreur") && !html.contains("erreur") {
            return Ok(Some(Vec::new()));
        }
    }
    Ok(None)
}

pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [item, head] = RE.get_or_init(|| {
        [
            Regex::new(r#"(?s)<li data-etat="([^"]*)" data-type="[^"]*">(.*?)</li>"#).expect("valid regex"),
            Regex::new(r#"(?s)^\s*<i[^>]*></i>\s*(.*?)\s*<a\s+href="([^"]*/instruction-(\d{4}-\d+))">([^<]*)</a>\s*(?:du\s*(\d\d-\d\d-\d{4}))?.*?<p>(.*?)</p>"#)
                .expect("valid regex"),
        ]
    });
    item.captures_iter(html)
        .filter_map(|li| {
            let c = head.captures(&li[2])?;
            let kind = clean(&c[1]);
            let signed = c.get(5).and_then(|d| french_date(d.as_str()));
            let subject = clean(&c[6]);
            let subject = subject.trim_end_matches(['.', ' ']);
            let title = match signed {
                Some(d) => format!(
                    "{kind} {} du {} : {subject}",
                    clean(&c[4]),
                    super::french_long(d)
                ),
                None => format!("{kind} {} : {subject}", clean(&c[4])),
            };
            Some(BulletinNotice {
                site: BulletinSite::Agri,
                native_id: c[3].to_string(),
                nor: None,
                signed,
                published: None,
                notice_url: c[2].to_string(),
                body_url: Some(format!("{}/telechargement", &c[2])),
                body: None,
                legifrance_uid: None,
                doctrine: matches!(
                    kind.as_str(),
                    "Note de service" | "Instruction technique" | "Instruction du Gouvernement"
                ),
                abrogated: matches!(&li[1], "Abrogee" | "Caduque"),
                kind,
                title,
            })
        })
        .collect()
}

/// The publication date and the NOR, which only the text's page gives, the
/// NOR when the text was also deposited with the Prime Minister's site.
pub(super) async fn enrich(client: &BulletinClient, notice: &mut BulletinNotice) -> Result<()> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [nor, published] = RE.get_or_init(|| {
        [
            Regex::new(r"N°\s*NOR\s*([A-Z]{4}\d{7}[A-Z])").expect("valid regex"),
            Regex::new(r"Publiée le\s*(\d\d-\d\d-\d{4})").expect("valid regex"),
        ]
    });
    let html = client.get_text(&notice.notice_url).await?;
    notice.nor = nor.captures(&html).and_then(|c| super::nor(&c[1]));
    notice.published = published
        .captures(&html)
        .and_then(|c| french_date(&c[1]))
        .or(notice.published);
    Ok(())
}

fn clean(s: &str) -> String {
    decode_entities(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn a_listed_instruction_gives_its_number_date_state_and_subject() {
        let html = r##"<ul class="liste-resultat">
            <li data-etat="Caduque" data-type="Instruction">
              <i class="icon-download-alt"></i>
              Instruction technique <a
                href="https://info.agriculture.gouv.fr/boagri/instruction-2019-875">DGAL/SDSSA/2019-875</a>
                du 27-12-2019
              <span class="badge-Caduque"><a href="#" class="info">Caduque<span>Le délai est dépassé.</span></a></span>
              <p>Plan de surveillance des mollusques bivalves - 2020. </p>
            </li>
            <li data-etat="EnVigueur" data-type="Instruction">
              <i class="icon-download-alt"></i>
              Note de mobilité <a href="https://info.agriculture.gouv.fr/boagri/instruction-2019-876">SG/SRH/2019-876</a>
                du 27-12-2019
              <p>Poste vacant.</p>
            </li></ul>"##;
        let notices = parse_listing(html);
        assert_eq!(notices.len(), 2);
        let n = &notices[0];
        assert_eq!(n.text_uid(), "bo_agri_2019-875");
        assert_eq!(
            n.title,
            "Instruction technique DGAL/SDSSA/2019-875 du 27 décembre 2019 : Plan de surveillance des mollusques bivalves - 2020"
        );
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2019, 12, 27));
        assert!(n.abrogated);
        assert!(n.doctrine);
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://info.agriculture.gouv.fr/boagri/instruction-2019-875/telechargement")
        );
        assert!(!notices[1].doctrine);
        assert!(!notices[1].abrogated);
    }
}
