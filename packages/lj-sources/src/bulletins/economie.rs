//! The economy and finance ministries' central administration bulletin, one
//! row per text since March 2021, grouped by issue date, and the texts their
//! « Documents opposables » page lists by directorate.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::{
    french_date, french_long_date, is_doctrine, nor, BulletinClient, BulletinNotice, BulletinSite,
};
use crate::error::Result;
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.economie.gouv.fr";
const LISTING: &str = "/bulletin-officiel/bulletin-officiel-de-ladministration-centrale";
const OPPOSABLES: &str = "/Documents-opposables";

/// A text both lists carry is the bulletin's.
pub(super) async fn list(client: &BulletinClient, since: NaiveDate) -> Result<Vec<BulletinNotice>> {
    let mut found = BTreeMap::new();
    // The listing runs oldest first, so it is cut at the source by its date filter.
    for page in 0.. {
        let html = client
            .get_text(&format!("{BASE_URL}{LISTING}?date_du={since}&page={page}"))
            .await?;
        let rows = parse_listing(&html);
        if rows.is_empty() {
            break;
        }
        for n in rows {
            found.entry(n.native_id.clone()).or_insert(n);
        }
    }
    let html = client.get_text(&format!("{BASE_URL}{OPPOSABLES}")).await?;
    for n in parse_opposables(&html)
        .into_iter()
        .filter(|n| n.published.or(n.signed).is_none_or(|d| d >= since))
    {
        found.entry(n.native_id.clone()).or_insert(n);
    }
    Ok(found.into_values().collect())
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

/// One table per directorate, one row per text: its type, its title linking the
/// document or the DILA circular, its reference, its date and the day it was
/// put online.
pub fn parse_opposables(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 6]> = OnceLock::new();
    let [row, cell, link, legifrance, size, date] = RE.get_or_init(|| {
        [
            Regex::new(r"(?s)<tr>\s*<th>\s*<p>\d+</p>\s*</th>(.*?)</tr>").expect("valid regex"),
            Regex::new(r"(?s)<td>(.*?)</td>").expect("valid regex"),
            Regex::new(r#"href="([^"]+)""#).expect("valid regex"),
            Regex::new(
                r"legifrance\.gouv\.fr/(?:circulaire/id/|download/pdf/circ\?id=|pdf/\d{4}/\d\d/cir_|index\.php\?\S*?[?&](?:amp;)?r=)(\d+)",
            )
            .expect("valid regex"),
            Regex::new(r"\((?i:pdf)[^)]*\)").expect("valid regex"),
            Regex::new(r"\d{1,2}/\d{1,2}/\d{4}").expect("valid regex"),
        ]
    });
    let mut out = Vec::new();
    for r in row.captures_iter(html) {
        let cells: Vec<&str> = cell
            .captures_iter(&r[1])
            .map(|c| c.get(1).expect("group").as_str())
            .collect();
        let [kind, title, reference, signed, online] = cells[..] else {
            continue;
        };
        // A few links carry their scheme twice, `http://https://…`.
        let href = link.captures(title).map(|c| {
            let h = decode_entities(&c[1]);
            match h.strip_prefix("http://") {
                Some(rest) if rest.starts_with("http") => rest.to_string(),
                _ => h,
            }
        });
        let legifrance_uid = href
            .as_deref()
            .and_then(|h| legifrance.captures(h))
            .map(|c| format!("cir_{}", &c[1]));
        let url = href.map(|h| {
            if h.starts_with('/') {
                format!("{BASE_URL}{h}")
            } else {
                h
            }
        });
        let pdf = url
            .clone()
            .filter(|u| legifrance_uid.is_none() && u.to_lowercase().ends_with(".pdf"));
        let reference = text(reference);
        let nor = nor(&reference);
        let Some(native_id) = nor
            .clone()
            .or_else(|| pdf.as_deref().map(file_stem))
            .or_else(|| legifrance_uid.clone())
        else {
            continue;
        };
        let kind = text(kind);
        let title = text(&size.replace_all(title, ""));
        let dated = |s: &str| {
            date.find(s)
                .and_then(|m| french_date(m.as_str()))
                .or_else(|| french_long_date(s))
        };
        out.push(BulletinNotice {
            site: BulletinSite::Economie,
            native_id,
            nor,
            doctrine: is_doctrine(&kind) || is_doctrine(&title),
            signed: dated(signed),
            published: dated(online),
            notice_url: url.unwrap_or_else(|| format!("{BASE_URL}{OPPOSABLES}")),
            body_url: pdf,
            body: None,
            legifrance_uid,
            abrogated: false,
            kind,
            title,
        });
    }
    out
}

fn text(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let tags = RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("valid regex"));
    decode_entities(&tags.replace_all(html, " "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn file_stem(url: &str) -> String {
    let name = url.rsplit('/').next().unwrap_or(url);
    let name = name.split('?').next().unwrap_or(name);
    name.strip_suffix(".pdf")
        .or_else(|| name.strip_suffix(".PDF"))
        .unwrap_or(name)
        .to_string()
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

    #[test]
    fn an_opposable_row_points_to_its_circular_or_its_document() {
        let html = r#"<tbody><tr>
<th>
<p>1</p>
</th>
<td>
<p>Circulaire</p>
</td>
<td>
<p><a href="https://www.legifrance.gouv.fr/circulaire/id/42842?init=true&amp;page=1&amp;query=ECOT1735688C" title="legifrance.gouv.fr">Circulaire du 15 décembre 2017</a> relative à la procédure de traitement des situations de surendettement des particuliers</p>
</td>
<td>
<p>ECOT1735688C</p>
</td>
<td>
<p>15/12/2017</p>
</td>
<td>
<p><em>20/12/2018</em></p>
</td>
</tr>
<tr>
<th>
<p>2</p>
</th>
<td>
<p>Note</p>
</td>
<td>
<p><a href="/files/files/PDF/2018/2017013710-attachment.pdf" data-entity-type="file">Information des agents de la DGFIP sur le harcèlement sexuel et moral</a> (pdf - 44 Ko)</p>
</td>
<td>
<p>2017/11/2743</p>
</td>
<td>
<p>27/11/2017</p>
</td>
<td>
<p><em>20/12/2018</em></p>
</td>
</tr>
<tr>
<th>
<p>3</p>
</th>
<td>
<p>Décision + Annexe</p>
</td>
<td>
<p><a href="/files/files/2025/D%C3%A9cision_delegation_signature_DI_ARA_SOMIF_du_1er_septembre_2025.pdf">Décision de délégation de signature du directeur interrégional</a>-Alpes [pdf-113Ko]</p>
</td>
<td>&nbsp;</td>
<td>
<p>1/09/2025</p>
</td>
<td>
<p>09/092025</p>
</td>
</tr></tbody>"#;
        let rows = parse_opposables(html);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].signed, NaiveDate::from_ymd_opt(2025, 9, 1));
        assert_eq!(rows[2].published, None);
        let c = &rows[0];
        assert_eq!(c.text_uid(), "bo_economie_ECOT1735688C");
        assert_eq!(c.legifrance_uid.as_deref(), Some("cir_42842"));
        assert_eq!(c.body_url, None);
        assert_eq!(c.signed, NaiveDate::from_ymd_opt(2017, 12, 15));
        assert_eq!(c.published, NaiveDate::from_ymd_opt(2018, 12, 20));
        let n = &rows[1];
        assert_eq!(n.text_uid(), "bo_economie_2017013710-attachment");
        assert_eq!(
            n.title,
            "Information des agents de la DGFIP sur le harcèlement sexuel et moral"
        );
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://www.economie.gouv.fr/files/files/PDF/2018/2017013710-attachment.pdf")
        );
        assert!(n.doctrine);
    }
}
