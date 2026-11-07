//! Bulletin officiel of the ecological transition ministries.

use std::sync::OnceLock;

use chrono::Datelike;
use regex::Regex;

use super::{french_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::Result;
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.bulletin-officiel.developpement-durable.gouv.fr";
const PAGE_SIZE: usize = 100;

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let mut out = Vec::new();
    for year in since_year..=chrono::Utc::now().year() {
        for page in 1.. {
            let url = format!(
                "{BASE_URL}/recherche?sort=date_publication_from&order=desc&hpp={PAGE_SIZE}&p={page}&annee_publication={year}"
            );
            let notices = parse_listing(&client.get_text(&url).await?);
            let last = notices.len() < PAGE_SIZE;
            out.extend(notices);
            if last {
                break;
            }
        }
    }
    Ok(out)
}

pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [head, field, doc, legifrance] = RE.get_or_init(|| {
        [
            Regex::new(r#"(?s)<a class="TITRE LIEN"\s+href="notice\?id=Bulletinofficiel-(\d+)[^"]*">\s*<span>(.*?)</span>"#)
                .expect("valid regex"),
            Regex::new(r#"(?s)<span class="(NUMERO-NOR|TYPE|DATE_SIGNATURE|DATE_PARUTION)">(.*?)</span>"#)
                .expect("valid regex"),
            Regex::new(r#"(?s)href="(documents/[^";"]+)[^"]*"[^>]*>\s*([^<]*)</a>"#).expect("valid regex"),
            Regex::new(r#"href="https?://(?:www\.)?(?:circulaire\.)?legifrance\.gouv\.fr/[^"]*?(?:[?&](?:amp;)?r=|/circulaire/id/)(\d+)"#)
                .expect("valid regex"),
        ]
    });
    let starts: Vec<_> = head.captures_iter(html).collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let whole = c.get(0).expect("match");
            let end = starts
                .get(i + 1)
                .map_or(html.len(), |n| n.get(0).expect("match").start());
            let block = &html[whole.end()..end];
            let mut notice = BulletinNotice {
                site: BulletinSite::Mte,
                native_id: c[1].to_string(),
                nor: None,
                kind: String::new(),
                title: clean(&c[2]),
                signed: None,
                published: None,
                notice_url: format!("{BASE_URL}/notice?id=Bulletinofficiel-{}", &c[1]),
                body_url: None,
                body: None,
                legifrance_uid: legifrance.captures(block).map(|l| format!("cir_{}", &l[1])),
                abrogated: false,
                doctrine: false,
            };
            for f in field.captures_iter(block) {
                let value = clean(&f[2]);
                let date = || value.rsplit(':').next().and_then(french_date);
                match &f[1] {
                    "NUMERO-NOR" => notice.nor = super::nor(&value),
                    "TYPE" => notice.kind = value.clone(),
                    "DATE_SIGNATURE" => notice.signed = date(),
                    _ => notice.published = date(),
                }
            }
            let docs: Vec<_> = doc.captures_iter(block).collect();
            notice.body_url = docs
                .iter()
                .find(|d| d[2].contains("principal"))
                .or(docs.first())
                .map(|d| format!("{BASE_URL}/{}", &d[1]));
            notice.doctrine = matches!(
                notice.kind.as_str(),
                "Circulaire" | "Instruction" | "Note" | "Directive"
            );
            notice
        })
        .collect()
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
    fn a_listing_entry_gives_its_nor_dates_and_main_document() {
        let html = r#"
            <a class="TITRE LIEN"
               href="notice?id=Bulletinofficiel-0034749&amp;reqId=839b&amp;pos=1">
               <span>Circulaire du 4 septembre 2026 relative &agrave; la gestion de la circulation routi&egrave;re.</span>
            </a>
            <div class="detailsResultat">
              <span class="NUMERO-NOR">TRAT2533039C</span>
              <span class="THEME">Transports</span>
              <span class="TYPE">Circulaire</span>
              <span class="DATE_SIGNATURE">Date de signature : 04-09-2026</span>
              <span class="DATE_PARUTION">Date de publication : 15-10-2026</span>
            </div>
            <a class="notice-lien" href="documents/Bulletinofficiel-0034749/annexe.pdf;jsessionid=AB12" title="annexe.pdf"> Annexe 1</a>
            <a class="notice-lien" href="documents/Bulletinofficiel-0034749/TRAT2533039C.pdf;jsessionid=AB12" title="TRAT2533039C.pdf"> Document
               principal</a>
            <a class="TITRE LIEN" href="notice?id=Bulletinofficiel-0034750"><span>Instruction du 1er octobre 2020</span></a>
            <span class="TYPE">Instruction</span>
            <a class="notice-lien" href="http://circulaire.legifrance.gouv.fr/index.php?action=afficherCirculaire&amp;hit=1&amp;r=44752"> Lien</a>"#;
        let notices = parse_listing(html);
        assert_eq!(notices.len(), 2);
        let n = &notices[0];
        assert_eq!(n.text_uid(), "bo_mte_0034749");
        assert_eq!(n.nor.as_deref(), Some("TRAT2533039C"));
        assert!(n
            .title
            .starts_with("Circulaire du 4 septembre 2026 relative à la gestion"));
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 9, 4));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 10, 15));
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://www.bulletin-officiel.developpement-durable.gouv.fr/documents/Bulletinofficiel-0034749/TRAT2533039C.pdf")
        );
        assert!(n.doctrine);
        assert_eq!(n.legifrance_uid, None);
        assert_eq!(notices[1].nor, None);
        assert_eq!(notices[1].body_url, None);
        assert_eq!(notices[1].legifrance_uid.as_deref(), Some("cir_44752"));
    }
}
