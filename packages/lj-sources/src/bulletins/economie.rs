//! The economy and finance ministries' central administration bulletin, one
//! row per text since March 2021, grouped by issue date.

use std::sync::OnceLock;

use regex::Regex;

use super::{french_long_date, is_doctrine, nor, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::Result;
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.economie.gouv.fr";
const LISTING: &str = "/bulletin-officiel/bulletin-officiel-de-ladministration-centrale";

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let mut out = Vec::new();
    // The listing runs oldest first, so it is cut at the source by its date filter.
    for page in 0.. {
        let html = client
            .get_text(&format!(
                "{BASE_URL}{LISTING}?date_du={since_year}-01-01&page={page}"
            ))
            .await?;
        let rows = parse_listing(&html);
        if rows.is_empty() {
            break;
        }
        out.extend(rows);
    }
    Ok(out)
}

pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [issue, row] = RE.get_or_init(|| {
        [
            Regex::new(r#"<p class="date">([^<]+)</p>"#).expect("valid regex"),
            Regex::new(r#"(?s)<td class="texte">\s*<a href="([^"]+)"[^>]*>\s*(.*?)\s*</a>.*?</td>\s*<td>\s*([A-Z0-9]*)\s*</td>"#)
                .expect("valid regex"),
        ]
    });
    let starts: Vec<_> = issue.captures_iter(html).collect();
    let mut out = Vec::new();
    for (k, c) in starts.iter().enumerate() {
        let start = c.get(0).expect("whole match").end();
        let end = starts
            .get(k + 1)
            .map_or(html.len(), |n| n.get(0).expect("whole match").start());
        let published = french_long_date(&c[1]);
        for r in row.captures_iter(&html[start..end]) {
            let title = decode_entities(&r[2])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let href = decode_entities(&r[1]);
            let url = format!("{BASE_URL}{href}");
            let nor = nor(&r[3]);
            let stem = href
                .rsplit('/')
                .next()
                .unwrap_or(&href)
                .trim_end_matches(".pdf")
                .to_string();
            out.push(BulletinNotice {
                site: BulletinSite::Economie,
                native_id: nor.clone().unwrap_or(stem),
                nor,
                kind: title.split_whitespace().next().unwrap_or("").to_string(),
                signed: title
                    .split_once(" du ")
                    .and_then(|(_, d)| french_long_date(d)),
                published,
                body_url: href.ends_with(".pdf").then(|| url.clone()),
                notice_url: url,
                body: None,
                legifrance_uid: None,
                doctrine: is_doctrine(&title),
                abrogated: false,
                title,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn each_row_takes_the_date_of_its_issue() {
        let html = r#"<div class="group">
    <p class="date">BOAC du 27 octobre 2026</p>
    <table><tbody>
        <tr>
          <td class="texte">
                      <a href="/files/actes-BOAC/2026-10/PMET2616007J_0_0.pdf" target="_blank">
                              Instruction interministérielle n° PMET2616007J du 15 octobre 2026 relative au suivi
                          </a>
                      (pdf - 133 ko)
          </td>
          <td>PMET2616007J</td>
          <td>Ministère de l&#039;action et des comptes publics</td>
        </tr></tbody></table></div>
      <div class="group">
    <p class="date">BOAC du 21 août 2026</p>
    <table><tbody><tr>
          <td class="texte"><a href="/boac/downloadfile/1234" target="_blank">Note du 3 août 2026 relative aux agents</a></td>
          <td>ECOP2619021N</td>
          <td>x</td></tr></tbody></table></div>"#;
        let rows = parse_listing(html);
        assert_eq!(rows.len(), 2);
        let n = &rows[0];
        assert_eq!(n.text_uid(), "bo_economie_PMET2616007J");
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 10, 27));
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 10, 15));
        assert_eq!(n.kind, "Instruction");
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://www.economie.gouv.fr/files/actes-BOAC/2026-10/PMET2616007J_0_0.pdf")
        );
        assert_eq!(rows[1].published, NaiveDate::from_ymd_opt(2026, 8, 21));
        assert_eq!(rows[1].body_url, None);
    }
}
