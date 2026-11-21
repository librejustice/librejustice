//! The foreign ministry's bulletin, listed by publication day, newest first:
//! one PDF per text named after its NOR, quarterly issues in its first years.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::issue::{cut_issue, signed_in_title};
use super::{is_doctrine, nor, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.diplomatie.gouv.fr";
const LISTING: &str =
    "/fr/le-ministere/publications-programmes-et-documentation-institutionnelle/bulletins-officiels";

pub(super) async fn list(client: &BulletinClient, since: NaiveDate) -> Result<Vec<BulletinNotice>> {
    let mut found = BTreeMap::new();
    let mut issues = BTreeMap::new();
    for page in 0.. {
        let html = client
            .get_text(&format!("{BASE_URL}{LISTING}?page={page}"))
            .await?;
        let rows = parse_listing(&html);
        let last = rows.is_empty() || rows.iter().any(|n| n.published.is_some_and(|d| d < since));
        for n in rows
            .into_iter()
            .filter(|n| n.published.is_none_or(|d| d >= since))
        {
            if n.title.starts_with("Bulletin officiel") {
                issues.insert(n.notice_url.clone(), n.published);
            } else {
                found.entry(n.native_id.clone()).or_insert(n);
            }
        }
        if last {
            break;
        }
    }
    for (url, published) in issues {
        let Some(bytes) = client.get_bytes(&url).await? else {
            tracing::warn!(issue = %url, "meae: issue pdf gone");
            continue;
        };
        let plain = tokio::task::spawn_blocking(move || crate::pdf::pdftotext_extract(&bytes))
            .await
            .map_err(|e| SourceError::Invalid(format!("pdftotext task: {e}")))??;
        let texts = cut_issue(BulletinSite::Meae, &url, &plain, published);
        if texts.is_empty() {
            tracing::warn!(issue = %url, "meae: issue cut to no text");
        }
        for n in texts {
            found.entry(n.native_id.clone()).or_insert(n);
        }
    }
    Ok(found.into_values().collect())
}

/// Each day's cards follow its heading, a title cut short past a length.
pub fn parse_listing(html: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [day, card, link, detail] = RE.get_or_init(|| {
        [
            Regex::new(r#"<h3>Bulletins officiels du <time datetime="(\d{4}-\d\d-\d\d)"#)
                .expect("valid regex"),
            Regex::new(r"(?s)<article\b.*?</article>").expect("valid regex"),
            Regex::new(r#"(?s)<h3 class="fr-card__title">\s*<a href="([^"]+)"[^>]*>(.*?)</a>"#)
                .expect("valid regex"),
            Regex::new(r#"<p class="fr-card__detail">\s*([^<]*?)\s*</p>"#).expect("valid regex"),
        ]
    });
    let days: Vec<_> = day.captures_iter(html).collect();
    let mut out = Vec::new();
    for (k, d) in days.iter().enumerate() {
        let start = d.get(0).expect("whole match").end();
        let end = days
            .get(k + 1)
            .map_or(html.len(), |n| n.get(0).expect("whole match").start());
        let published = NaiveDate::parse_from_str(&d[1], "%Y-%m-%d").ok();
        for a in card.find_iter(&html[start..end]) {
            let Some(l) = link.captures(a.as_str()) else {
                continue;
            };
            let title = decode_entities(&l[2])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let title = title
                .strip_prefix("Télécharger le texte ")
                .unwrap_or(&title)
                .to_string();
            let href = decode_entities(&l[1]);
            let url = if href.starts_with('/') {
                format!("{BASE_URL}{href}")
            } else {
                href
            };
            let nor = detail.captures_iter(a.as_str()).find_map(|c| nor(&c[1]));
            let native_id = nor.clone().unwrap_or_else(|| file_stem(&url));
            out.push(BulletinNotice {
                site: BulletinSite::Meae,
                native_id,
                nor,
                kind: title.split_whitespace().next().unwrap_or("").to_string(),
                signed: signed_in_title(&title),
                published,
                notice_url: url.clone(),
                body_url: Some(url),
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

fn file_stem(url: &str) -> String {
    let name = url.rsplit('/').next().unwrap_or(url);
    name.strip_suffix(".pdf").unwrap_or(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_text_takes_its_day_its_nor_and_its_document() {
        let html = r#"<p class="fr-h6">1172 arrêté(s)</p>
<div class="fr-col-12 fr-col-md-4">
        <h3>Bulletins officiels du <time datetime="2026-10-29T12:00:00Z" class="datetime">29 octobre 2026</time>
</h3></div>
<article class="fr-col-12"><div class="fr-card__content">
  <h3 class="fr-card__title">
    <a href="/files/bulletins_officiels/EAEF2628831A.pdf" download="">
      Télécharger le texte Arrêté du 28 octobre 2026 relatif aux fonctions exercées par le consul honoraire de France à Torreón (Mexique)
    </a>
  </h3>
  <div class="fr-card__start"><ul class="fr-tags-group"><li><p class="fr-tag">MEAE</p></li></ul>
        <p class="fr-card__detail">EAEF2628831A</p></div>
  <div class="fr-card__end"><p class="fr-card__detail">PDF - 48.7 KB

</p></div></div></article>
        <h3>Bulletins officiels du <time datetime="2007-04-01T12:00:00Z" class="datetime">01 avril 2007</time>
</h3>
<article class="fr-col-12"><div class="fr-card__content">
  <h3 class="fr-card__title">
    <a href="/files/files/le-ministere/bulletin-officiel/no_099_avril-juin_2007_cle068a98-1.pdf" download="">
      Télécharger le texte Bulletin officiel n°99 (avril-juin)
    </a>
  </h3>
  <div class="fr-card__end"><p class="fr-card__detail">PDF - 1 351.2 KB</p></div></div></article>"#;
        let rows = parse_listing(html);
        assert_eq!(rows.len(), 2);
        let n = &rows[0];
        assert_eq!(n.text_uid(), "bo_meae_EAEF2628831A");
        assert_eq!(
            n.title,
            "Arrêté du 28 octobre 2026 relatif aux fonctions exercées par le consul honoraire de France à Torreón (Mexique)"
        );
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 10, 28));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 10, 29));
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://www.diplomatie.gouv.fr/files/bulletins_officiels/EAEF2628831A.pdf")
        );
        assert_eq!(n.nature(), Some("ARRETE"));
        let issue = &rows[1];
        assert_eq!(issue.title, "Bulletin officiel n°99 (avril-juin)");
        assert_eq!(issue.nor, None);
        assert_eq!(issue.published, NaiveDate::from_ymd_opt(2007, 4, 1));
    }
}
