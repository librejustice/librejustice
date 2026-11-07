//! Bulletin officiel of the justice ministry, listed by publication date.

use std::sync::OnceLock;

use chrono::Datelike;
use regex::Regex;

use super::{french_long_date, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::Result;
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.justice.gouv.fr";
const PAGE_SIZE: usize = 100;

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let mut out = Vec::new();
    // Newest first, so cut on the dates read rather than through the site's `start` filter.
    for page in 0.. {
        let url = format!(
            "{BASE_URL}/documentation/bulletin-officiel?items_per_page={PAGE_SIZE}&page={page}"
        );
        let notices = parse_listing(&client.get_text(&url).await?);
        let last = notices.len() < PAGE_SIZE
            || notices
                .iter()
                .any(|n| n.published.is_some_and(|d| d.year() < since_year));
        out.extend(
            notices
                .into_iter()
                .filter(|n| n.published.is_none_or(|d| d.year() >= since_year)),
        );
        if last {
            break;
        }
    }
    Ok(out)
}

/// The cards follow the heading of the bulletin that published them.
pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 6]> = OnceLock::new();
    let [token, title, tag, nor, pdf, page] = RE.get_or_init(|| {
        [
            Regex::new(r#"(?s)<h2\s+class="fr-title"\s*>(.*?)</h2>|<article class="fr-card\s*">(.*?)</article>"#)
                .expect("valid regex"),
            Regex::new(r#"(?s)class="fr-card__title[^"]*"\s*>(.*?)</h2>"#).expect("valid regex"),
            Regex::new(r#"(?s)<p class="fr-tag">(.*?)</p>"#).expect("valid regex"),
            Regex::new(r"NOR\s*:\s*([A-Za-z0-9]+)").expect("valid regex"),
            Regex::new(r#"href="([^"]+\.pdf)""#).expect("valid regex"),
            Regex::new(r#"href="(/[^"]+)"[^>]*class="mj-content-link"#).expect("valid regex"),
        ]
    });
    let start = html.find("mj-official-texts-cards__content").unwrap_or(0);
    let mut published = None;
    let mut out = Vec::new();
    for t in token.captures_iter(&html[start..]) {
        if let Some(heading) = t.get(1) {
            published = french_long_date(&clean(heading.as_str()));
            continue;
        }
        let card = &t[2];
        let Some(path) = page.captures(card).map(|c| c[1].to_string()) else {
            continue;
        };
        let title = title
            .captures(card)
            .map(|c| clean(&c[1]))
            .unwrap_or_default();
        let kind = tag.captures(card).map(|c| clean(&c[1])).unwrap_or_default();
        out.push(BulletinNotice {
            site: BulletinSite::Justice,
            native_id: path.rsplit('/').next().unwrap_or(&path).to_string(),
            nor: nor.captures(card).and_then(|c| super::nor(&c[1])),
            signed: signed_in_title(&title),
            published,
            notice_url: format!("{BASE_URL}{path}"),
            body_url: pdf.captures(card).map(|c| absolute(&c[1])),
            body: None,
            legifrance_uid: None,
            abrogated: false,
            doctrine: matches!(
                kind.as_str(),
                "Circulaire" | "Instruction" | "Note" | "Dépêche" | "Guide"
            ),
            kind,
            title,
        });
    }
    out
}

/// « Circulaire du 4 mars 2025 relative à … »: the date the title opens with.
fn signed_in_title(title: &str) -> Option<chrono::NaiveDate> {
    let head: String = title.chars().take(80).collect();
    let (before, after) = head.split_once(" du ")?;
    (before.split_whitespace().count() <= 4)
        .then(|| french_long_date(after))
        .flatten()
}

fn absolute(href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else {
        format!("{BASE_URL}{href}")
    }
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
    fn a_card_takes_the_date_of_the_bulletin_heading_it_follows() {
        let html = r#"<div class="mj-official-texts-cards__content">
            <h2 class="fr-title" >Bulletin officiel du 26 octobre 2026</h2>
            <article class="fr-card ">
              <h2 class="fr-card__title fr-title" > Circulaire du 1er octobre 2026 relative &agrave; la politique p&eacute;nale </h2>
              <p class="fr-tag">Circulaire </p>
              <p class="fr-card__detail  "> NOR : JUSD2628463C </p>
              <a href="https://www.justice.gouv.fr/sites/default/files/2026-10/JUSD2628463C.pdf" class="fr-link">PDF</a>
              <a href="/documentation/bulletin-officiel/circulaire-politique-penale" title="Lire la suite" class="mj-content-link fr-link">Lire la suite</a>
            </article>
            <h2 class="fr-title" >Bulletin officiel du 3 septembre 2026</h2>
            <article class="fr-card ">
              <h2 class="fr-card__title fr-title" >Note relative aux frais de justice</h2>
              <p class="fr-tag">Note </p>
              <p class="fr-card__detail  "> NOR : H </p>
              <a href="/documentation/bulletin-officiel/note-frais" class="mj-content-link fr-link">Lire la suite</a>
            </article>"#;
        let notices = parse_listing(html);
        assert_eq!(notices.len(), 2);
        let n = &notices[0];
        assert_eq!(n.text_uid(), "bo_justice_circulaire-politique-penale");
        assert_eq!(n.nor.as_deref(), Some("JUSD2628463C"));
        assert_eq!(
            n.title,
            "Circulaire du 1er octobre 2026 relative à la politique pénale"
        );
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 10, 1));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 10, 26));
        assert!(n.doctrine);
        let n = &notices[1];
        assert_eq!(n.nor, None);
        assert_eq!(n.signed, None);
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 9, 3));
        assert_eq!(n.body_url, None);
    }
}
