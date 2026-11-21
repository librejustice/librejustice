//! The ACPR's official register: its listing gives each act's collection and day, the act's page its PDF.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::{french_long_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::Result;
use crate::html_strip::{decode_entities, strip_html};

pub const BASE_URL: &str = "https://acpr.banque-france.fr";
const REGISTER: &str = "/fr/reglementation/registre-officiel";
const PAGE_SIZE: usize = 12;

/// The register lists its acts newest first.
pub(super) async fn list(client: &BulletinClient, since: NaiveDate) -> Result<Vec<BulletinNotice>> {
    let mut found = BTreeMap::new();
    for page in 0.. {
        let notices = parse_listing(
            &client
                .get_text(&format!("{BASE_URL}{REGISTER}?page={page}"))
                .await?,
        );
        let last = notices.len() < PAGE_SIZE
            || notices
                .iter()
                .any(|n| n.published.is_some_and(|d| d < since));
        for n in notices
            .into_iter()
            .filter(|n| n.published.is_none_or(|d| d >= since))
        {
            found.entry(n.native_id.clone()).or_insert(n);
        }
        if last {
            break;
        }
    }
    Ok(found.into_values().collect())
}

pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [card, collection] = RE.get_or_init(|| {
        [
            Regex::new(r#"(?s)href ="(/fr/publications-et-statistiques/publications/([^"/]+))".*?<h3[^>]*>(.*?)</h3>(.*?)</a>"#)
                .expect("valid regex"),
            Regex::new(r"Registre officiel - ([^|]+?)\s*\|").expect("valid regex"),
        ]
    });
    card.captures_iter(html)
        .filter_map(|c| {
            let rest = clean(&strip_html(&c[4].replace('<', "|<")));
            let kind = collection.captures(&rest)?[1].trim().to_string();
            let title = clean(&c[3]);
            let title = match title.strip_suffix("[...]") {
                Some(cut) => format!("{}…", cut.trim_end()),
                None => title,
            };
            Some(BulletinNotice {
                site: BulletinSite::Acpr,
                native_id: c[2].to_string(),
                nor: None,
                signed: None,
                published: french_long_date(&rest),
                notice_url: format!("{BASE_URL}{}", &c[1]),
                body_url: None,
                body: None,
                legifrance_uid: None,
                doctrine: false,
                abrogated: false,
                kind,
                title,
            })
        })
        .collect()
}

/// The act's own PDF, the first its page offers.
pub(super) async fn enrich(client: &BulletinClient, notice: &mut BulletinNotice) -> Result<()> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let pdf = RE
        .get_or_init(|| Regex::new(r#"href ="(/system/files/[^"]+\.pdf)""#).expect("valid regex"));
    let html = client.get_text(&notice.notice_url).await?;
    notice.body_url = pdf
        .captures(&html)
        .map(|c| format!("{BASE_URL}{}", decode_entities(&c[1])));
    Ok(())
}

fn clean(s: &str) -> String {
    decode_entities(s)
        .split(|c: char| c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_gives_the_act_its_collection_day_and_page() {
        let html = r#"<a class="card card-vertical rounded"
          href ="/fr/publications-et-statistiques/publications/instruction-ndeg-2026-i-14-relative-au-plan"
          rel ="noopener">
	<div class="card-body"><h3 class="card-title h6">Instruction n° 2026-I-14 relative au plan de gestion du risque de liquidité [...]</h3>
	<span class="badge">Registre officiel - Instruction</span><span>Assurance</span>
	<p class="date">10 Juillet 2026</p></div></a>"#;
        let notices = parse_listing(html);
        assert_eq!(notices.len(), 1);
        let n = &notices[0];
        assert_eq!(
            n.text_uid(),
            "bo_acpr_instruction-ndeg-2026-i-14-relative-au-plan"
        );
        assert_eq!(n.kind, "Instruction");
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 7, 10));
        assert_eq!(
            n.title,
            "Instruction n° 2026-I-14 relative au plan de gestion du risque de liquidité…"
        );
        assert_eq!(n.notice_url, "https://acpr.banque-france.fr/fr/publications-et-statistiques/publications/instruction-ndeg-2026-i-14-relative-au-plan");
    }
}
