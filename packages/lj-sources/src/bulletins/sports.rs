//! The sports ministry's bulletin, monthly or bimonthly issues of 2012 to 2020.

use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::culture::title_at;
use super::issue::{cut_issue, running_heads, signed_in_title, strip_running_heads};
use super::{french_date, is_doctrine, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.sports.gouv.fr";
const ARCHIVE: &str = "/bulletin-officiel-jeunesse-sports-et-vie-associative-1976";
/// The bulletin merged into national education's in January 2021.
const LAST_YEAR: i32 = 2020;

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    if since_year > LAST_YEAR {
        return Ok(Vec::new());
    }
    let html = client.get_text(&format!("{BASE_URL}{ARCHIVE}")).await?;
    let mut out = Vec::new();
    for issue in issues(&html).into_iter().filter(|i| i.year >= since_year) {
        let Some(bytes) = client.get_bytes(&issue.pdf).await? else {
            tracing::warn!(issue = %issue.pdf, "sports: issue pdf gone");
            continue;
        };
        let plain = tokio::task::spawn_blocking(move || crate::pdf::pdftotext_extract(&bytes))
            .await
            .map_err(|e| SourceError::Invalid(format!("pdftotext task: {e}")))??;
        let mut texts = cut_issue(BulletinSite::Sports, &issue.pdf, &plain, issue.published);
        if texts.is_empty() {
            texts = cut_by_summary(&issue, &plain);
        }
        if texts.is_empty() {
            tracing::warn!(issue = %issue.pdf, "sports: issue cut to no text");
        }
        out.extend(texts);
    }
    Ok(out)
}

pub struct Issue {
    pub id: String,
    pub year: i32,
    pub pdf: String,
    pub published: Option<NaiveDate>,
}

/// An issue the archive gives no day online is dated the first of its month.
fn issues(html: &str) -> Vec<Issue> {
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [row, pdf, online] = RE.get_or_init(|| {
        [
            Regex::new(r"(?s)<tr>(.*?)</tr>").expect("valid regex"),
            Regex::new(r#"href="([^"]*/bo(\d+)-([a-z-]+)-(\d{4})-\d+\.pdf)""#)
                .expect("valid regex"),
            Regex::new(r"Mis en ligne le (\d{1,2}/\d{1,2}/\d{4})").expect("valid regex"),
        ]
    });
    row.captures_iter(html)
        .filter_map(|r| {
            let c = pdf.captures(&r[1])?;
            let year: i32 = c[4].parse().ok()?;
            let first_month = month(c[3].split('-').next().unwrap_or(""));
            let published = online
                .captures(&r[1])
                .and_then(|o| french_date(&o[1]))
                .or_else(|| NaiveDate::from_ymd_opt(year, first_month?, 1));
            Some(Issue {
                id: format!("{year}-{}", &c[2]),
                year,
                pdf: decode_entities(&c[1]),
                published,
            })
        })
        .collect()
}

/// The slug's month, its accented letters dropped (`d-cembre`, `f-vrier`).
fn month(slug: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "janvier",
        "f",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "aout",
        "septembre",
        "octobre",
        "novembre",
        "d",
    ];
    MONTHS.iter().position(|m| *m == slug).map(|k| k as u32 + 1)
}

/// The issues of 2012 and 2013 print no NOR; their summary's titles cut them.
fn cut_by_summary(issue: &Issue, plain: &str) -> Vec<BulletinNotice> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [opens, closes] = RE.get_or_init(|| {
        [
            Regex::new(r"^-?\s*(?:ARR[EÊ]T[EÉ]|D[EÉ]CISION|NOTE|CIRCULAIRE|INSTRUCTION|AVIS|PACTE|EXTRAIT|CONVENTION|D[EÉ]LIB[EÉ]RATION|LISTE|PROTOCOLE|CHARTE|ANNEXE|LETTRE|RECTIFICATIF)\b")
                .expect("valid regex"),
            Regex::new(r"\.{2,}\s*p\.\s*(\d+)\s*$").expect("valid regex"),
        ]
    });
    let pages: Vec<&str> = plain.split('\u{c}').collect();
    let front = pages
        .iter()
        .take_while(|p| {
            p.contains("RAPPEL DES TEXTES") || p.lines().any(|l| closes.is_match(l.trim()))
        })
        .count();
    let mut entries: Vec<(u32, String)> = Vec::new();
    let mut open: Option<String> = None;
    for line in pages[..front].iter().flat_map(|p| p.lines()) {
        let line = line.trim();
        let current = match open.take() {
            Some(title) => Some(title + " " + line),
            None if opens.is_match(line) => Some(line.trim_start_matches(['-', ' ']).to_string()),
            None => None,
        };
        match (current, closes.captures(line)) {
            (Some(title), Some(c)) => {
                let title = title[..title.len() - c[0].len().min(title.len())].trim();
                if let Ok(page) = c[1].parse() {
                    entries.push((page, title.trim_end_matches('.').to_string()));
                }
            }
            (current, _) => open = current,
        }
    }
    entries.sort_by_key(|(page, _)| *page);
    let body_from: usize = pages[..front].iter().map(|p| p.len() + 1).sum();
    let text = &plain[body_from.min(plain.len())..];
    let mut found: Vec<(usize, &String)> = Vec::new();
    let mut from = 0;
    for (_, title) in &entries {
        if let Some(at) = title_at(&text[from..], title) {
            found.push((from + at, title));
            from += at + 1;
        } else {
            tracing::info!(issue = %issue.pdf, title = %title, "sports: summary entry not found in its pages");
        }
    }
    let running = running_heads(plain);
    found
        .iter()
        .enumerate()
        .map(|(k, &(at, title))| {
            let end = found.get(k + 1).map_or(text.len(), |n| n.0);
            let page = plain[..body_from + at].matches('\u{c}').count();
            BulletinNotice {
                site: BulletinSite::Sports,
                native_id: format!("{}-{}", issue.id, k + 1),
                nor: None,
                kind: title.split_whitespace().next().unwrap_or("").to_string(),
                signed: signed_in_title(&title.to_lowercase()),
                published: issue.published,
                notice_url: format!("{}#page={}", issue.pdf, page + 1),
                body_url: None,
                body: Some(strip_running_heads(&text[at..end], &running)),
                legifrance_uid: None,
                doctrine: is_doctrine(title),
                abrogated: false,
                title: title.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_lists_each_issue_with_its_number_pdf_and_day_online() {
        let html = r#"<tr><td><strong>Décembre 2020</strong></td>
<td><strong>n°12</strong></td>
<td><a href="https://www.sports.gouv.fr/sites/default/files/2023-06/bo12-d-cembre-2020-6473.pdf" target="_blank">Version pdf</a></td>
<td>Mis en ligne le 06/01/2021</td>
</tr><tr><td><strong>Février 2012</strong></td>
<td><strong>n° 2</strong></td>
<td><a href="https://www.sports.gouv.fr/sites/default/files/2023-06/bo2-f-vrier-2012-6257.pdf" target="_blank">Version pdf</a></td>
</tr>"#;
        let issues = issues(html);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].id, "2020-12");
        assert_eq!(issues[0].published, NaiveDate::from_ymd_opt(2021, 1, 6));
        assert_eq!(issues[1].id, "2012-2");
        assert_eq!(issues[1].year, 2012);
        assert_eq!(issues[1].published, NaiveDate::from_ymd_opt(2012, 2, 1));
    }

    #[test]
    fn an_issue_without_nor_is_cut_on_its_summary_titles() {
        let plain = "Bulletin\nSOMMAIRE\nRappel des textes parus au Journal officiel......p. 2\nFORMATION\n\
            - NOTE D’INFORMATION N° DS/2012/303 DU 1er AOÛT 2012 relative au calendrier des stages\nde recyclage des guides.......p. 3\n\
            PERSONNEL\n- ARRETE DU 24 JUILLET 2012 portant désignation de l’agent chargé de la mission.......p. 4\n\u{c}\
            BBulletin\nRAPPEL DES TEXTES PARUS AU J.O.R.F.\n- Arrêté du 19 juillet 2012\n2\n\u{c}\
            BBulletin\nFORMATION\nNOTE D’INFORMATION N° DS/2012/303\nDU 1er AOÛT 2012\nrelative au calendrier des stages de recyclage des guides\nTexte.\n3\n\u{c}\
            BBulletin\nPERSONNEL\nARRETE DU 24 JUILLET 2012\nportant désignation de l’agent chargé de la mission\nArt. 1.\n4\n";
        let issue = Issue {
            id: "2012-7".into(),
            year: 2012,
            pdf: "https://x/bo7.pdf".into(),
            published: NaiveDate::from_ymd_opt(2012, 7, 1),
        };
        let texts = cut_by_summary(&issue, plain);
        assert_eq!(texts.len(), 2);
        let note = &texts[0];
        assert_eq!(note.text_uid(), "bo_sports_2012-7-1");
        assert_eq!(
            note.title,
            "NOTE D’INFORMATION N° DS/2012/303 DU 1er AOÛT 2012 relative au calendrier des stages de recyclage des guides"
        );
        assert!(note.doctrine);
        assert_eq!(note.signed, NaiveDate::from_ymd_opt(2012, 8, 1));
        assert_eq!(note.notice_url, "https://x/bo7.pdf#page=3");
        assert!(note
            .body
            .as_deref()
            .is_some_and(|b| b.ends_with("Texte.\nPERSONNEL")));
        let arrete = &texts[1];
        assert_eq!(arrete.nature(), Some("ARRETE"));
        assert!(arrete
            .body
            .as_deref()
            .is_some_and(|b| b.starts_with("ARRETE DU 24 JUILLET 2012") && b.ends_with("Art. 1.")));
    }
}
