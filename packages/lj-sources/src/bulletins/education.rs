//! The national education bulletin: the site map lists every text's page,
//! which gives its issue, its type, its signature and its body.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;

use super::{
    french_date, french_long, french_long_date, is_doctrine, BulletinClient, BulletinNotice,
    BulletinSite,
};
use crate::error::{Result, SourceError};
use crate::html_strip::{decode_entities, strip_html};

pub const BASE_URL: &str = "https://www.education.gouv.fr";
const SITEMAP: &str = "/edugouv-sitemap-3/sitemap.xml";

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    static LOC: OnceLock<Regex> = OnceLock::new();
    let loc = LOC.get_or_init(|| Regex::new(r"<loc>([^<]+)</loc>").expect("valid regex"));
    let index = client.get_text(&format!("{BASE_URL}{SITEMAP}")).await?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut texts = 0;
    for page in loc.captures_iter(&index) {
        let xml = client.get_text(&decode_entities(&page[1])).await?;
        let (count, notices) = parse_sitemap(&xml, since_year);
        texts += count;
        out.extend(
            notices
                .into_iter()
                .filter(|n| seen.insert(n.native_id.clone())),
        );
    }
    if texts == 0 {
        return Err(SourceError::Invalid(format!(
            "education: no bulletin text in {SITEMAP}"
        )));
    }
    Ok(out)
}

/// One page's texts since the year, and its text count.
pub fn parse_sitemap(xml: &str, since_year: i32) -> (usize, Vec<BulletinNotice>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"<loc>(https://www\.education\.gouv\.fr/bo/(\d{2,4})/[^/<]+/([A-Z]{4}\d{7}[A-Z])(?:\.htm)?)</loc>")
            .expect("valid regex")
    });
    let mut count = 0;
    let mut out = Vec::new();
    for c in re.captures_iter(xml) {
        count += 1;
        let year: i32 = c[2].parse().unwrap_or(0);
        let year = if year < 100 { 2000 + year } else { year };
        let nor = c[3].to_string();
        if year < since_year {
            continue;
        }
        out.push(BulletinNotice {
            site: BulletinSite::Education,
            native_id: nor.clone(),
            nor: Some(nor),
            kind: String::new(),
            title: String::new(),
            signed: None,
            published: None,
            notice_url: c[1].to_string(),
            body_url: None,
            body: None,
            legifrance_uid: None,
            doctrine: true,
            abrogated: false,
        });
    }
    (count, out)
}

pub(super) async fn enrich(client: &BulletinClient, notice: &mut BulletinNotice) -> Result<()> {
    let html = client.get_text(&notice.notice_url).await?;
    read_page(&html, notice)
}

pub fn read_page(html: &str, n: &mut BulletinNotice) -> Result<()> {
    static RE: OnceLock<[Regex; 5]> = OnceLock::new();
    let [issue, field, tag, heading, date] = RE.get_or_init(|| {
        [
            Regex::new(r#"<p class="bo-detail-title">\s*([^<]+?)\s*</p>"#).expect("valid regex"),
            Regex::new(r#"<p class="fr-mb-0">([^<:]+?)\s*:\s*<strong>([^<]*)</strong></p>"#)
                .expect("valid regex"),
            Regex::new(r#"(?s)class="fr-tag">\s*([^<]*?)\s*</p>\s*</p>"#).expect("valid regex"),
            Regex::new(r#"(?s)<h1 class="fr-mb-2w[^"]*">\s*(.*?)\s*</h1>"#).expect("valid regex"),
            Regex::new(r"(\d{1,2}-\d{1,2}-\d{4})").expect("valid regex"),
        ]
    });
    let layout =
        || SourceError::Invalid(format!("education: page layout changed {}", n.notice_url));
    let (start, end) = (
        html.find(r#"<div class="bo-detail-pre-content">"#),
        html.find(r#"<div class="fr-pt-5w fr-border-top">"#),
    );
    let (Some(start), Some(end)) = (start, end) else {
        return Err(layout());
    };
    let (label, value) = field
        .captures_iter(html)
        .map(|c| (clean(&c[1]), clean(&c[2])))
        .find(|(label, _)| label != "NOR" && label != "Emetteur")
        .unwrap_or_default();
    let rubric = tag.captures(html).map(|c| clean(&c[1]));
    let title = heading
        .captures(html)
        .map(|c| clean(&c[1]))
        .filter(|t| !t.is_empty());
    if rubric.is_none() && title.is_none() {
        return Err(layout());
    }
    let rubric = rubric.unwrap_or_default();
    let title = title.unwrap_or_else(|| rubric.clone());
    n.published = issue.captures(html).and_then(|c| french_long_date(&c[1]));
    n.signed = date.captures(&value).and_then(|c| french_date(&c[1]));
    let number = value
        .split_once(" du ")
        .map(|(number, _)| format!(" {number}"))
        .unwrap_or_default();
    let signed = n
        .signed
        .map(|d| format!(" du {}", french_long(d)))
        .unwrap_or_default();
    let head = format!("{label}{number}{signed}");
    let subject = if rubric.is_empty() || rubric == title {
        title
    } else {
        format!("{rubric} : {title}")
    };
    n.title = if head.is_empty() {
        subject
    } else {
        format!("{head}, {subject}")
    };
    n.doctrine = is_doctrine(&label);
    n.kind = label;
    n.body = Some(strip_html(&html[start..end.max(start)]));
    Ok(())
}

fn clean(s: &str) -> String {
    decode_entities(&strip_html(s))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn the_site_map_keeps_every_text_since_the_year() {
        let xml =
            "<url><loc>https://www.education.gouv.fr/bo/19/Hebdo23/MENE1915816C.htm</loc></url>\
<url><loc>https://www.education.gouv.fr/bo/2026/Hebdo41/MENE2626341N</loc></url>\
<url><loc>https://www.education.gouv.fr/bo/2026/Hebdo41/MENA2626429A</loc></url>\
<url><loc>https://www.education.gouv.fr/bo/15/Hebdo16/MENE1500236C.htm</loc></url>\
<url><loc>https://www.education.gouv.fr/node/274424</loc></url>";
        let (count, notices) = parse_sitemap(xml, 2019);
        assert_eq!(count, 4);
        let ids: Vec<_> = notices.iter().map(BulletinNotice::text_uid).collect();
        assert_eq!(
            ids,
            [
                "bo_education_MENE1915816C",
                "bo_education_MENE2626341N",
                "bo_education_MENA2626429A"
            ]
        );
    }

    #[test]
    fn a_text_page_gives_its_type_dates_title_and_body() {
        let html = r#"<div class="fr-hero-top__content">
          <p class="bo-detail-title">Bulletin officiel n° 41 du 29 octobre 2026</p>
          </div>
    <div class="hero">
  <p  data-component-id="edugouv_dsfr:texte_as_title" class="fr-h5">
  <p data-component-id="ui_suite_dsfr:tag" class="fr-tag">
    Concours général des lycées
  </p>
</p>
            <h1 class="fr-mb-2w fr-mb-md-3w">
    Organisation de la session 2027
    </h1>
<div class="bo-detail-pre-content">
      <p class="fr-mb-0">NOR : <strong>MENE2624777N</strong></p>
      <p class="fr-mb-0">Note de service : <strong>du 5-10-2026</strong></p>
      <p class="fr-mb-0">Emetteur : <strong>MEN - DGESCO A - MPE</strong></p>
  </div>
<div class="body"><p>La présente note de service a pour objet&nbsp;de préciser.</p></div>
<div class="fr-pt-5w fr-border-top"> <div class="fr-share">Partager</div></div>"#;
        let (_, mut notices) = parse_sitemap(
            "<loc>https://www.education.gouv.fr/bo/2026/Hebdo41/MENE2624777N</loc>",
            2019,
        );
        let n = &mut notices[0];
        read_page(html, n).unwrap();
        assert_eq!(n.kind, "Note de service");
        assert!(n.doctrine);
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 10, 5));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 10, 29));
        assert_eq!(
            n.title,
            "Note de service du 5 octobre 2026, Concours général des lycées : Organisation de la session 2027"
        );
        let body = n.body.as_deref().unwrap();
        assert!(body.starts_with("NOR : MENE2624777N"));
        assert!(body.ends_with("a pour objet de préciser."));
    }

    #[test]
    fn a_text_without_a_heading_is_titled_by_its_rubric() {
        let html = r#"<p class="bo-detail-title">Bulletin officiel spécial n° 2 du 13 février 2020</p>
<p class="fr-tag">Épreuve de l'enseignement de spécialité de la classe de terminale</p>

</p>
</div>
<div class="bo-detail-pre-content"><p class="fr-mb-0">NOR : <strong>MENE2001794N</strong></p>
<p class="fr-mb-0">Note de service : <strong>n° 2020-027 du 11-2-2020</strong></p></div><p>Texte.</p>
<div class="fr-pt-5w fr-border-top">"#;
        let (_, mut notices) = parse_sitemap(
            "<loc>https://www.education.gouv.fr/bo/20/Special2/MENE2001794N.htm</loc>",
            2019,
        );
        let n = &mut notices[0];
        read_page(html, n).unwrap();
        assert_eq!(
            n.title,
            "Note de service n° 2020-027 du 11 février 2020, Épreuve de l'enseignement de spécialité de la classe de terminale"
        );
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2020, 2, 13));
    }

    #[test]
    fn a_text_without_a_rubric_is_titled_by_its_heading() {
        let html = r#"<p class="bo-detail-title">Bulletin officiel spécial n° 4 du 17 septembre 2026</p>
<p  data-component-id="edugouv_dsfr:texte_as_title" class="fr-h5">
</p>
<h1 class="fr-mb-2w fr-mb-md-3w">
    Épreuves terminales anticipées de français de la classe de première
    </h1>
<div class="bo-detail-pre-content"><p class="fr-mb-0">NOR : <strong>MENE2622658N</strong></p>
<p class="fr-mb-0">Note de service : <strong>du 11-9-2026</strong></p></div><p>Texte.</p>
<div class="fr-pt-5w fr-border-top">"#;
        let (_, mut notices) = parse_sitemap(
            "<loc>https://www.education.gouv.fr/bo/2026/Special4/MENE2622658N</loc>",
            2019,
        );
        let n = &mut notices[0];
        read_page(html, n).unwrap();
        assert_eq!(
            n.title,
            "Note de service du 11 septembre 2026, Épreuves terminales anticipées de français de la classe de première"
        );
        assert!(n.doctrine);
    }

    #[test]
    fn a_text_without_a_type_line_is_titled_by_its_heading_and_set_aside() {
        let html = r#"<p class="bo-detail-title">Bulletin officiel n° 32 du 3 septembre 2020</p>
<p class="fr-tag">Classes préparatoires aux grandes écoles</p>

</p>
<h1 class="fr-mb-2w fr-mb-md-3w">Liste des CPGE pour l&#039;année universitaire 2020-2021</h1>
<div class="bo-detail-pre-content"><p class="fr-mb-0">NOR : <strong>ESRS2018324K</strong></p>
<p class="fr-mb-0">Emetteur : <strong>MESRI - DGESIP-A1-2</strong></p></div><p>Texte.</p>
<div class="fr-pt-5w fr-border-top">"#;
        let (_, mut notices) = parse_sitemap(
            "<loc>https://www.education.gouv.fr/bo/20/Hebdo32/ESRS2018324K.htm</loc>",
            2019,
        );
        let n = &mut notices[0];
        read_page(html, n).unwrap();
        assert_eq!(
            n.title,
            "Classes préparatoires aux grandes écoles : Liste des CPGE pour l'année universitaire 2020-2021"
        );
        assert_eq!(n.kind, "");
        assert!(!n.doctrine);
    }

    #[test]
    fn a_numbered_circular_keeps_its_number() {
        let html = r#"<p class="bo-detail-title">Bulletin officiel n°23 du 6 juin 2019</p>
<p class="fr-tag">Circulaire de rentrée 2019 - École inclusive</p></p><h1 class="fr-mb-2w">Pour une École inclusive</h1>
<div class="bo-detail-pre-content"><p class="fr-mb-0">NOR : <strong>MENE1915816C</strong></p>
<p class="fr-mb-0">Circulaire : <strong>n° 2019-088 du 5-6-2019</strong></p></div><p>Texte.</p>
<div class="fr-pt-5w fr-border-top">"#;
        let (_, mut notices) = parse_sitemap(
            "<loc>https://www.education.gouv.fr/bo/19/Hebdo23/MENE1915816C.htm</loc>",
            2019,
        );
        let n = &mut notices[0];
        read_page(html, n).unwrap();
        assert_eq!(
            n.title,
            "Circulaire n° 2019-088 du 5 juin 2019, Circulaire de rentrée 2019 - École inclusive : Pour une École inclusive"
        );
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2019, 6, 6));
    }
}
