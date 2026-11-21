//! The customs bulletin: its listing gives each administrative decision's number, title, day and abrogation, its page its PDF.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::{french_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::{decode_entities, strip_html};

pub const BASE_URL: &str = "https://www.douane.gouv.fr";
const LISTING: &str = "/bulletins-officiels-des-douanes";
const DECISION: &str = "/la-douane/informations/bulletins-officiels-des-douanes/da/";

/// The listing runs by bulletin number, newest first, two to fifteen decisions a page.
pub(super) async fn list(client: &BulletinClient, since: NaiveDate) -> Result<Vec<BulletinNotice>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let total = RE.get_or_init(|| {
        Regex::new(r#"data-fr-analytics-page-total="(\d+)""#).expect("valid regex")
    });
    let mut found = BTreeMap::new();
    let mut pages = 1;
    let mut page = 0;
    while page < pages {
        let html = client
            .get_text(&format!("{BASE_URL}{LISTING}?page={page}"))
            .await?;
        if page == 0 {
            pages = total
                .captures(&html)
                .and_then(|c| c[1].parse().ok())
                .ok_or_else(|| {
                    SourceError::Invalid("douane: listing states no page count".into())
                })?;
        }
        let notices = parse_listing(&html);
        if notices.is_empty() {
            return Err(SourceError::Invalid(format!(
                "douane: listing page {page} holds no decision"
            )));
        }
        let older = notices
            .iter()
            .all(|n| n.published.is_some_and(|d| d < since));
        for n in notices
            .into_iter()
            .filter(|n| n.published.is_none_or(|d| d >= since))
        {
            found.entry(n.native_id.clone()).or_insert(n);
        }
        if older {
            break;
        }
        page += 1;
    }
    Ok(found.into_values().collect())
}

pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [number, title, day, abrogated] = RE.get_or_init(|| {
        [
            Regex::new(r#"/bod/\d+">\s*BOD\s+\d+\s+([^\s<]+)\s*</a>"#).expect("valid regex"),
            Regex::new(r#"(?s)class="fr-card__desc">(.*?)</p>"#).expect("valid regex"),
            Regex::new(r#"class="fr-card__detail[^>]*>\s*([0-9/]+)"#).expect("valid regex"),
            Regex::new(r#"role="tooltip">\s*Abrogée? par"#).expect("valid regex"),
        ]
    });
    html.split(r#"class="fr-card__start""#)
        .skip(1)
        .filter_map(|card| {
            let id = number.captures(card)?[1].to_string();
            let mut n = BulletinNotice {
                site: BulletinSite::Douane,
                notice_url: format!("{BASE_URL}{DECISION}{id}"),
                native_id: id,
                nor: None,
                kind: String::new(),
                title: clean(&strip_html(&title.captures(card)?[1])),
                signed: None,
                published: day.captures(card).and_then(|c| french_date(&c[1])),
                body_url: None,
                body: None,
                legifrance_uid: None,
                doctrine: false,
                abrogated: abrogated.is_match(card),
            };
            // An administrative decision of no other nature is an instruction to the services.
            n.doctrine = n.nature().is_none();
            Some(n)
        })
        .collect()
}

/// The decision's date and its own PDF, the first its page offers, the annexes after it.
pub(super) async fn enrich(client: &BulletinClient, notice: &mut BulletinNotice) -> Result<()> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [signed, pdf] = RE.get_or_init(|| {
        [
            Regex::new(r">Du</strong>\s*</div>\s*<div[^>]*>\s*([0-9/]+)").expect("valid regex"),
            Regex::new(r#"href="(/sites/default/files/[^"]+\.pdf)""#).expect("valid regex"),
        ]
    });
    let html = client.get_text(&notice.notice_url).await?;
    notice.signed = signed.captures(&html).and_then(|c| french_date(&c[1]));
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
    fn a_card_gives_the_decision_its_number_day_and_abrogation() {
        let html = r#"<div class="fr-card__start"> <p class="fr-text-sm"> J.4.3.1 - Le régime de l&#039;avitaillement des bateaux </p> </div>
          <h2 class="fr-card__title"> <a href="http://www.douane.gouv.fr/la-douane/informations/bulletins-officiels-des-douanes/bod/7272"> BOD 7272
                       18-062 </a> <span class="fr-badge" role="status" aria-label="Abroge">ABROGÉ</span>
          <span class="fr-tooltip fr-placement tooltip-weight" id="tooltip-7272-0" role="tooltip">Abrogé par la DA 19-008</span> </h2>
          <p class="fr-card__desc">Régime fiscal des produits énergétiques destinés à la navigation maritime</p>
          <div class="fr-card__end"> <p class="fr-card__detail fr-mt-auto">20/11/2018</p> </div>
          <div class="fr-card__start"> <p class="fr-text-sm"> R - Contributions indirectes </p> </div>
          <h2 class="fr-card__title"> <a href="http://www.douane.gouv.fr/la-douane/informations/bulletins-officiels-des-douanes/bod/7273"> BOD 7273 18-063 </a>
          <span class="fr-tooltip fr-placement tooltip-weight" id="tooltip-7273-0" role="tooltip">Abroge la DA 17-039</span> </h2>
          <p class="fr-card__desc">Décision du 16 novembre 2018 portant délégation de signature</p>
          <div class="fr-card__end"> <p class="fr-card__detail fr-mt-auto">12/11/2018</p> </div>"#;
        let notices = parse_listing(html);
        assert_eq!(notices.len(), 2);
        let n = &notices[0];
        assert_eq!(n.text_uid(), "bo_douane_18-062");
        assert_eq!(
            n.notice_url,
            "https://www.douane.gouv.fr/la-douane/informations/bulletins-officiels-des-douanes/da/18-062"
        );
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2018, 11, 20));
        assert!(n.abrogated);
        assert!(n.doctrine);
        let n = &notices[1];
        assert!(!n.abrogated);
        assert!(!n.doctrine);
        assert_eq!(n.nature(), Some("DECISION"));
    }
}
