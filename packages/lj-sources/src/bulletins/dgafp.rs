//! The civil service directorate's circulars, one page listing them by year,
//! each link a Légifrance circular or a PDF of the site's own.

use std::sync::OnceLock;

use regex::Regex;

use super::{french_long_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::Result;
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.fonction-publique.gouv.fr";
const PAGE: &str = "/droit-de-la-fonction-publique/actualites-du-droit-de-la-fonction-publique/circulaires-fonction-publique";

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let html = client.get_text(&format!("{BASE_URL}{PAGE}")).await?;
    Ok(parse_page(&html, since_year))
}

pub fn parse_page(html: &str, since_year: i32) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 6]> = OnceLock::new();
    let [section, item, link, tags, nor, legifrance] = RE.get_or_init(|| {
        [
            Regex::new(r#"id="annee(\d{4}|_anterieures)""#).expect("valid regex"),
            Regex::new(r"(?s)<li>(.*?)</li>").expect("valid regex"),
            Regex::new(r#"<a href="([^"]+)""#).expect("valid regex"),
            Regex::new(r"<[^>]+>").expect("valid regex"),
            Regex::new(r"NOR\s*:?\s*([A-Z]{4}\d{7}[A-Z])").expect("valid regex"),
            Regex::new(
                r"legifrance\.gouv\.fr/(?:download/pdf/circ\?id=|circulaire/id/|pdf/\d{4}/\d\d/cir_)(\d+)",
            )
                .expect("valid regex"),
        ]
    });
    let starts: Vec<(usize, Option<i32>)> = section
        .captures_iter(html)
        .filter_map(|c| Some((c.get(0)?.start(), c[1].parse().ok())))
        .collect();
    let earliest = starts
        .iter()
        .filter_map(|&(_, y)| y)
        .min()
        .unwrap_or(i32::MAX);
    let mut out = Vec::new();
    for (k, &(start, year)) in starts.iter().enumerate() {
        if year.unwrap_or(earliest - 1) < since_year {
            continue;
        }
        let next = starts.get(k + 1).map_or(html.len(), |&(e, _)| e);
        let end = html[start..next].find("</ul>").map_or(next, |e| start + e);
        for li in item.captures_iter(&html[start..end]) {
            let Some(href) = link.captures(&li[1]).map(|c| decode_entities(&c[1])) else {
                continue;
            };
            let title = decode_entities(&tags.replace_all(&li[1], ""))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let nor = nor.captures(&title).and_then(|c| super::nor(&c[1]));
            let legifrance_uid = legifrance.captures(&href).map(|c| format!("cir_{}", &c[1]));
            let url = if href.starts_with('/') {
                format!("{BASE_URL}{href}")
            } else {
                href.clone()
            };
            let native_id = nor.clone().unwrap_or_else(|| file_stem(&href));
            out.push(BulletinNotice {
                site: BulletinSite::Dgafp,
                native_id,
                nor,
                kind: title.split_whitespace().next().unwrap_or("").to_string(),
                signed: title
                    .split_once(" du ")
                    .and_then(|(_, d)| french_long_date(d)),
                published: None,
                body_url: (legifrance_uid.is_none() && href.ends_with(".pdf")).then(|| url.clone()),
                notice_url: url,
                body: None,
                legifrance_uid,
                doctrine: true,
                abrogated: false,
                title,
            });
        }
    }
    out
}

fn file_stem(href: &str) -> String {
    let name = href.rsplit('/').next().unwrap_or(href);
    let name = name.strip_suffix(".pdf").unwrap_or(name);
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn a_local_pdf_is_kept_and_a_legifrance_link_set_aside() {
        let html = r#"<h2 class="fr-h2"><strong id="annee2026">Année 2026</strong></h2>
<ul>
<li><a href="/files/files/Textes%20de%20r%C3%A9f%C3%A9f%C3%A9rence/2026/20261006161057.pdf" title="Lien externe" target="_blank">Circulaire PM du 1<sup>er</sup> octobre 2026</a> relative à la formation des agents publics</li>
<li><a href="/files/files/droit-de-la-fonction-publique/CPPF2619494C_0_0.pdf" id title="x">Circulaire n° NOR CPPF2619494C du 7 août 2026</a> relative au dispositif de rupture conventionnelle</li>
<li><a href="https://www.legifrance.gouv.fr/download/pdf/circ?id=45665" title="x">Circulaire n° NOR CPPF2605968C du 12 mai 2026</a> relative à la prestation</li>
<li><a href="http://circulaires.legifrance.gouv.fr/pdf/2026/08/cir_45032.pdf">Circulaire du 3 août 2026</a> relative à un sujet</li>
</ul>
<h2 class="fr-h2"><strong id="annee2018">Année 2018</strong></h2>
<ul><li><a href="/files/old.pdf">Circulaire du 3 mars 2018</a> relative à un ancien sujet</li></ul>
<h2 class="fr-h2"><strong id="annee_anterieures">Années antérieures</strong></h2>
<ul><li><a href="/files/C_19990423_FP0001.pdf">FP du 23 avril 1999</a></li></ul>
</main><footer><ul><li><a href="/les-flux-rss">Flux RSS</a></li></ul></footer>"#;
        assert_eq!(parse_page(html, 2018).len(), 5);
        let all = parse_page(html, 0);
        assert_eq!(all.len(), 6);
        assert_eq!(all[5].text_uid(), "bo_dgafp_C-19990423-FP0001");
        let notices = parse_page(html, 2019);
        assert_eq!(notices.len(), 4);
        let n = &notices[0];
        assert_eq!(n.text_uid(), "bo_dgafp_20261006161057");
        assert_eq!(
            n.title,
            "Circulaire PM du 1er octobre 2026 relative à la formation des agents publics"
        );
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 10, 1));
        assert!(n
            .body_url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://www.fonction-publique.gouv.fr/files/files/")));
        assert_eq!(notices[1].text_uid(), "bo_dgafp_CPPF2619494C");
        assert_eq!(notices[1].nor.as_deref(), Some("CPPF2619494C"));
        assert_eq!(notices[2].legifrance_uid.as_deref(), Some("cir_45665"));
        assert_eq!(notices[2].body_url, None);
        assert_eq!(notices[3].legifrance_uid.as_deref(), Some("cir_45032"));
    }
}
