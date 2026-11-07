//! The interior ministry's bulletin, two issues a month listed by the
//! documentation page, each text its own PDF named after its NOR since 2021,
//! one PDF per issue before.

use std::sync::OnceLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;

use super::issue::cut_issue;
use super::{french_long_date, is_doctrine, nor, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.interieur.gouv.fr";
const LISTING: &str = "/documentation?document_category%5B277%5D=277";

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let mut issues = Vec::new();
    for page in 0.. {
        let html = client
            .get_text(&format!("{BASE_URL}{LISTING}&page={page}"))
            .await?;
        let listed = parse_listing(&html);
        let older = listed.iter().any(|(_, d)| d.year() < since_year);
        let empty = listed.is_empty();
        issues.extend(listed.into_iter().filter(|(_, d)| d.year() >= since_year));
        if older || empty {
            break;
        }
    }
    let mut out = Vec::new();
    for (path, published) in issues {
        let url = format!("{BASE_URL}{path}");
        let html = client.get_text(&url).await?;
        let texts = parse_issue(&html, &url, published);
        if texts.is_empty() {
            out.extend(whole_issue(client, &url, &html, published).await?);
        }
        out.extend(texts);
    }
    Ok(out)
}

async fn whole_issue(
    client: &BulletinClient,
    issue_url: &str,
    html: &str,
    published: NaiveDate,
) -> Result<Vec<BulletinNotice>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let pdf = RE.get_or_init(|| {
        Regex::new(r#"href="(/sites/minint/files/[^"]+\.pdf)""#).expect("valid regex")
    });
    let pdfs: Vec<String> = pdf
        .captures_iter(html)
        .map(|c| format!("{BASE_URL}{}", &c[1]))
        .collect();
    if pdfs.is_empty() {
        tracing::warn!(issue = %issue_url, "interieur: issue without a document");
    }
    let mut out = Vec::new();
    for url in pdfs {
        let Some(bytes) = client.get_bytes(&url).await? else {
            tracing::warn!(issue = %url, "interieur: issue pdf gone");
            continue;
        };
        let plain = tokio::task::spawn_blocking(move || crate::pdf::pdftotext_extract(&bytes))
            .await
            .map_err(|e| SourceError::Invalid(format!("pdftotext task: {e}")))??;
        out.extend(cut_issue(
            BulletinSite::Interieur,
            &url,
            &plain,
            Some(published),
        ));
    }
    Ok(out)
}

/// The issues of one listing page and their publication dates.
pub fn parse_listing(html: &str) -> Vec<(String, NaiveDate)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?s)<a href="(/documentation/bulletin-officiel-du-ministere-de-linterieur/[^"]+)".*?Publié le\s*<em[^>]*>([^<]+)</em>"#)
            .expect("valid regex")
    });
    re.captures_iter(html)
        .filter_map(|c| Some((c[1].to_string(), french_long_date(&c[2])?)))
        .collect()
}

pub fn parse_issue(html: &str, issue_url: &str, published: NaiveDate) -> Vec<BulletinNotice> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"download="([^"]*)"\s*href="(/sites/minint/files/[^"]+/[A-Z0-9]+_(\d{8})_([^/"]+)\.pdf)""#)
            .expect("valid regex")
    });
    re.captures_iter(html)
        .map(|c| {
            let title = decode_entities(&c[1])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let nor = nor(&c[4]);
            BulletinNotice {
                site: BulletinSite::Interieur,
                native_id: nor.clone().unwrap_or_else(|| c[4].to_string()),
                nor,
                kind: title.split_whitespace().next().unwrap_or("").to_string(),
                signed: NaiveDate::parse_from_str(&c[3], "%Y%m%d").ok(),
                published: Some(published),
                notice_url: issue_url.to_string(),
                body_url: Some(format!("{BASE_URL}{}", &c[2])),
                body: None,
                legifrance_uid: None,
                doctrine: is_doctrine(&title),
                abrogated: false,
                title,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_lists_each_text_with_its_document() {
        let listing = r#"<h3 class="fr-card__title">
        <a href="/documentation/bulletin-officiel-du-ministere-de-linterieur/bomi-octobre-2026-ndeg-1-du-16-octobre-2026.html"  target="_self" class="fr-card__link">
          BOMI Octobre 2026 n° 1 du 16 octobre 2026
        </a></h3>
        <p class="fr-card__detail">Publié le <em class="placeholder">16 octobre 2026</em></p>"#;
        let issues = parse_listing(listing);
        assert_eq!(issues.len(), 1);
        let published = issues[0].1;
        assert_eq!(published, NaiveDate::from_ymd_opt(2026, 10, 16).unwrap());

        let html = r#"<a id="link-91224" data-fr-assess-file download="Arrêté du 13 octobre 2026 fixant la liste des formations laïcité, religion et citoyenneté"
   href="/sites/minint/files/medias/documents/2026-10/E00_20261013_INTD2621877A.pdf"
   class="fr-link">
<a id="link-91227" data-fr-assess-file download="Circulaire du 28 septembre 2026 relative au dispositif d&#039;aide d'urgence aux exploitants agricoles"
   href="/sites/minint/files/medias/documents/2026-10/H00_20260928_MOMO2625799C.pdf"
   class="fr-link">"#;
        let notices = parse_issue(html, "https://www.interieur.gouv.fr/x.html", published);
        assert_eq!(notices.len(), 2);
        assert!(!notices[0].doctrine);
        let n = &notices[1];
        assert!(n.doctrine);
        assert_eq!(n.text_uid(), "bo_interieur_MOMO2625799C");
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 9, 28));
        assert_eq!(
            n.title,
            "Circulaire du 28 septembre 2026 relative au dispositif d'aide d'urgence aux exploitants agricoles"
        );
        assert_eq!(
            n.body_url.as_deref(),
            Some("https://www.interieur.gouv.fr/sites/minint/files/medias/documents/2026-10/H00_20260928_MOMO2625799C.pdf")
        );
    }
}
