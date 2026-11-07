//! The health and labour bulletins, one site since July 2025 publishing each
//! text on its own page, before it one PDF per issue on each ministry's site.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::issue::{cut_issue, signed_in_title};
use super::{french_long_date, site_dir, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://bulletins-officiels.social.gouv.fr";
const INSTRUCTION: (u32, &str) = (217, "Instruction");
const CIRCULAIRE: (u32, &str) = (280, "Circulaire");
const NOTE: (u32, &str) = (286, "Note");
const NOTE_INFO: (u32, &str) = (223, "Note d'information");
/// A type a bulletin does not offer answers 500.
const BULLETINS: [(&str, &[(u32, &str)]); 2] = [
    (
        "bo-sante-protection-sociale-solidarites",
        &[INSTRUCTION, CIRCULAIRE, NOTE, NOTE_INFO],
    ),
    (
        "bo-travail-emploi-formation-professionnelle",
        &[INSTRUCTION, CIRCULAIRE, NOTE],
    ),
];
const PAGE_SIZE: usize = 3;
const HEALTH: &str = "https://sante.gouv.fr";
/// Links each year's page, a few under a suffixed slug.
const HEALTH_YEARS: &str = "/ministere/documentation-et-publications-officielles/bulletins-officiels-et-documents-opposables/article/bulletin-officiel-sante-protection-sociale-solidarite";
const LABOUR: &str = "https://travail-emploi.gouv.fr";
/// The issues stopped in June 2025, so a run reaching back a single year
/// leaves them alone.
const ARCHIVE_LAST_YEAR: i32 = 2025;

/// A text both ministries signed sits in both bulletins under one page.
pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let since = NaiveDate::from_ymd_opt(since_year, 1, 1).expect("valid date");
    let mut found = BTreeMap::new();
    for (bulletin, types) in BULLETINS {
        for &(type_id, kind) in types {
            for page in 0.. {
                let url = format!(
                    "{BASE_URL}/{bulletin}?field_institutional_target_id={type_id}&page={page}"
                );
                let notices = parse_listing(&client.get_text(&url).await?, kind);
                let last = notices.len() < PAGE_SIZE
                    || notices
                        .iter()
                        .any(|n| n.published.is_some_and(|d| d < since));
                for n in notices {
                    found.insert(n.native_id.clone(), n);
                }
                if last {
                    break;
                }
            }
        }
    }
    let mut out: Vec<_> = found.into_values().collect();
    if since_year < ARCHIVE_LAST_YEAR {
        out.extend(archive(client, since_year).await?);
    }
    Ok(out)
}

pub fn parse_listing(html: &str, kind: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [card, link, nor, published] = RE.get_or_init(|| {
        [
            Regex::new(r#"(?s)<li class="view-row[^"]*">(.*?)</li>"#).expect("valid regex"),
            Regex::new(r#"(?s)<h3 class="fr-card__title">\s*<a href="/([^"]+)">(.*?)</a>"#)
                .expect("valid regex"),
            Regex::new(r"NOR\s*:\s*([A-Za-z0-9]+)").expect("valid regex"),
            Regex::new(r#"BO du\s*<time datetime="(\d{4}-\d\d-\d\d)"#).expect("valid regex"),
        ]
    });
    card.captures_iter(html)
        .filter_map(|c| {
            let card = &c[1];
            let l = link.captures(card)?;
            let title = clean(&l[2]);
            Some(BulletinNotice {
                site: BulletinSite::Social,
                native_id: l[1].to_string(),
                nor: nor.captures(card).and_then(|c| super::nor(&c[1])),
                kind: kind.to_string(),
                signed: signed_in_title(&title),
                published: published
                    .captures(card)
                    .and_then(|c| NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").ok()),
                notice_url: format!("{BASE_URL}/{}", &l[1]),
                body_url: None,
                body: None,
                legifrance_uid: None,
                doctrine: true,
                abrogated: false,
                title,
            })
        })
        .collect()
}

/// The PDF, which only the text's page links, and the NOR a card lacked.
pub(super) async fn enrich(client: &BulletinClient, notice: &mut BulletinNotice) -> Result<()> {
    if notice.body.is_some() {
        return Ok(());
    }
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [pdf, nor] = RE.get_or_init(|| {
        [
            Regex::new(r#"href="(/sites/textes-officiels/files/[^"]+\.pdf)""#)
                .expect("valid regex"),
            Regex::new(r"NOR\s*:\s*([A-Z]{4}\d{7}[A-Z])").expect("valid regex"),
        ]
    });
    let html = client.get_text(&notice.notice_url).await?;
    notice.body_url = pdf.captures(&html).map(|c| format!("{BASE_URL}{}", &c[1]));
    if notice.nor.is_none() {
        notice.nor = nor.captures(&html).and_then(|c| super::nor(&c[1]));
    }
    Ok(())
}

async fn archive(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let pdf = RE.get_or_init(|| Regex::new(r#"href="([^"]+\.pdf)""#).expect("valid regex"));
    let mut found = BTreeMap::new();
    for (host, page) in year_pages(client, since_year).await? {
        let html = client.get_text(&page).await?;
        let issues: BTreeSet<String> = pdf
            .captures_iter(&html)
            .map(|c| format!("{host}/{}", c[1].trim_start_matches('/')))
            .collect();
        for url in issues {
            let Some(bytes) = frozen_issue(client, &url).await? else {
                tracing::warn!(issue = %url, "social: issue pdf gone");
                continue;
            };
            let plain = tokio::task::spawn_blocking(move || crate::pdf::pdftotext_extract(&bytes))
                .await
                .map_err(|e| SourceError::Invalid(format!("pdftotext task: {e}")))??;
            for n in cut_issue(BulletinSite::Social, &url, &plain, published_in(&plain)) {
                found.entry(n.native_id.clone()).or_insert(n);
            }
        }
    }
    Ok(found.into_values().collect())
}

/// The archive is frozen: an issue read once is kept beside the site's state,
/// so an interrupted run resumes where it stopped.
async fn frozen_issue(client: &BulletinClient, url: &str) -> Result<Option<Vec<u8>>> {
    let dir = site_dir(&client.cache, BulletinSite::Social).join("issues");
    let path = dir.join(url.trim_start_matches("https://").replace('/', "_"));
    match tokio::fs::read(&path).await {
        Ok(bytes) => return Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let Some(bytes) = client.get_bytes(url).await? else {
        return Ok(None);
    };
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(&path, &bytes).await?;
    Ok(Some(bytes))
}

async fn year_pages(
    client: &BulletinClient,
    since_year: i32,
) -> Result<Vec<(&'static str, String)>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let link = RE.get_or_init(|| {
        Regex::new(r##"href="/?([^"#]*/bulletins-officiels-sante-protection-sociale-solidarite-(\d{4})(?:-\d+)?)""##)
            .expect("valid regex")
    });
    let years = since_year..=ARCHIVE_LAST_YEAR;
    let html = client.get_text(&format!("{HEALTH}{HEALTH_YEARS}")).await?;
    let health: BTreeMap<i32, String> = link
        .captures_iter(&html)
        .filter_map(|c| Some((c[2].parse().ok()?, format!("{HEALTH}/{}", &c[1]))))
        .filter(|(y, _)| years.contains(y))
        .collect();
    if let Some(y) = years.clone().find(|y| !health.contains_key(y)) {
        return Err(SourceError::Invalid(format!(
            "social: no health bulletin page for {y}"
        )));
    }
    Ok(health
        .into_values()
        .map(|p| (HEALTH, p))
        .chain(years.map(|y| (LABOUR, format!("{LABOUR}/bulletin-officiel-annee-{y}"))))
        .collect())
}

/// The date the running head repeats on every page, « Bulletin officiel Santé
/// - … n° 2024/34 du 31 décembre 2024 », « BO Travail no 2019/5 du 30 mai 2019 ».
fn published_in(plain: &str) -> Option<NaiveDate> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?:BO|Bulletin officiel)\b[^\n]{0,120}?\bn[oº°]\s?\d{4}/\d+[^\n]*? du ([^\n,]+)",
        )
        .expect("valid regex")
    });
    let mut seen = BTreeMap::new();
    for d in re
        .captures_iter(plain)
        .filter_map(|c| french_long_date(&c[1]))
    {
        *seen.entry(d).or_insert(0) += 1;
    }
    seen.into_iter().max_by_key(|&(_, n)| n).map(|(d, _)| d)
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

    #[test]
    fn a_card_gives_its_page_nor_and_bulletin_date() {
        let html = r#"<ul class="view-list fr-raw-list">
      <li class="view-row fr-col-12 fr-col-lg-4 fr-col-md-6">
<div about="/circulaire-ndeg-dgosfip12026136-du-12-octobre-2026" data-component-id="dsfr4drupal:card" class="fr-enlarge-link fr-card">
      <h3 class="fr-card__title">
                              <a href="/circulaire-ndeg-dgosfip12026136-du-12-octobre-2026">Circulaire n° DGOS/FIP1/2026/136 du 12 octobre 2026 relative à la deuxième phase de délégation des crédits</a>
                        </h3>
  <p data-component-id="dsfr4drupal:tag" data-component-variant="sm" class="fr-tag">
    NOR : SFHH2624079C
  </p>
                      <p class="fr-card__detail ">BO du     <time datetime="2026-10-12T15:14:03Z">12 octobre 2026</time>
</p>
</li>
      <li class="view-row fr-col-12">
      <h3 class="fr-card__title">
                              <a href="/note-relative-aux-modalites">Note relative aux modalités</a>
                        </h3>
                      <p class="fr-card__detail ">BO du     <time datetime="2026-09-30T12:00:00Z">30 septembre 2026</time>
</li></ul>"#;
        let notices = parse_listing(html, "Circulaire");
        assert_eq!(notices.len(), 2);
        let n = &notices[0];
        assert_eq!(
            n.text_uid(),
            "bo_social_circulaire-ndeg-dgosfip12026136-du-12-octobre-2026"
        );
        assert_eq!(n.nor.as_deref(), Some("SFHH2624079C"));
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2026, 10, 12));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2026, 10, 12));
        assert_eq!(
            n.notice_url,
            "https://bulletins-officiels.social.gouv.fr/circulaire-ndeg-dgosfip12026136-du-12-octobre-2026"
        );
        assert_eq!(notices[1].nor, None);
        assert_eq!(notices[1].signed, None);
    }

    #[test]
    fn an_archive_issue_is_cut_on_the_nor_under_each_title() {
        let head = "Bulletin officiel Santé - Protection sociale - Solidarité n° 2024/34 du 31 décembre 2024";
        let plain = [
            "Sommaire\ntexte initial publié au Bulletin officiel Santé - Protection sociale Solidarité n° 2024/33 du 16 décembre 2024).\nPage 1",
            "Ministère du travail et de l’emploi\nArrêté du 4 novembre 2024 portant composition de la commission\nNOR : MSAR2430452A\nArticle 1er\nPage 2",
            "Ministère de la santé et de l’accès aux soins\nINSTRUCTION INTERMINISTÉRIELLE N° DSS/2B/2024/181 du 20 décembre 2024\nrelative à la revalorisation des plafonds\nNOR : SAES2434195J\nPremier alinéa.\nPage 3",
            "Second alinéa.\n42\nPage 4",
            "Caisse nationale de l’assurance maladie\nDécision du 16 décembre 2024 portant délégation\nNOR : MSAX2430556S\nPage 5",
            "INSTRUCTION INTERMINISTÉRIELLE N° DSS/2B/2024/181 du 20 décembre 2024\nNOR : SAES2434195J\nAnnexe.\nPage 6",
        ]
        .map(|page| format!("{head}\n{page}\n"))
        .join("\u{c}");
        let notices = cut_issue(
            BulletinSite::Social,
            "https://x/bo.pdf",
            &plain,
            published_in(&plain),
        );
        let uids: Vec<_> = notices.iter().map(BulletinNotice::text_uid).collect();
        assert_eq!(
            uids,
            [
                "bo_social_MSAR2430452A",
                "bo_social_SAES2434195J",
                "bo_social_MSAX2430556S"
            ]
        );
        assert_eq!(
            notices[2].title,
            "Décision du 16 décembre 2024 portant délégation"
        );
        let n = &notices[1];
        assert!(n.doctrine);
        assert_eq!(
            n.title,
            "Instruction interministérielle n° DSS/2B/2024/181 du 20 décembre 2024 relative à la revalorisation des plafonds"
        );
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2024, 12, 20));
        assert_eq!(n.published, NaiveDate::from_ymd_opt(2024, 12, 31));
        assert_eq!(n.notice_url, "https://x/bo.pdf#page=3");
        assert_eq!(
            n.body.as_deref(),
            Some("INSTRUCTION INTERMINISTÉRIELLE N° DSS/2B/2024/181 du 20 décembre 2024\nrelative à la revalorisation des plafonds\nNOR : SAES2434195J\nPremier alinéa.\nSecond alinéa.\n\nINSTRUCTION INTERMINISTÉRIELLE N° DSS/2B/2024/181 du 20 décembre 2024\nNOR : SAES2434195J\nAnnexe.")
        );
    }
}
