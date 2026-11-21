//! The culture ministry's bulletin: one PDF per text since February 2026,
//! monthly issues before, cut by their table of contents.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use chrono::{DateTime, Datelike, NaiveDate};
use regex::Regex;
use serde::Deserialize;

use super::{french_long_date, is_doctrine, BulletinClient, BulletinNotice, BulletinSite};
use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.culture.gouv.fr";
const ARCHIVE: &str = "/rechercher-une-publication-du-ministere-de-la-culture/bulletin-officiel";
/// The monthly issues stopped in January 2026, so a run reaching back a
/// single year leaves them alone.
const LAST_ISSUE_YEAR: i32 = 2026;

pub(super) async fn list(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    let mut out = per_text(client, since_year).await?;
    if since_year < LAST_ISSUE_YEAR - 1 {
        out.extend(issues(client, since_year).await?);
    }
    Ok(out)
}

#[derive(Deserialize)]
struct SearchPage {
    hits: Vec<Hit>,
    #[serde(rename = "nbPages")]
    nb_pages: u32,
}

#[derive(Deserialize)]
struct Hit {
    #[serde(rename = "objectID")]
    object_id: String,
    url: String,
    official_bulletin_name_value: String,
    official_bulletin_nor_value: Option<String>,
    content_publication_date: i64,
}

/// The site's search index and its public key, read from the bulletins page and
/// the script it loads.
async fn per_text(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [script, index, key] = RE.get_or_init(|| {
        [
            Regex::new(r#"/assets/themes/gouv/js/algoliaLazyLoader\.[0-9a-f]+\.js"#)
                .expect("valid regex"),
            Regex::new(r#""indexList":\{"default":"([^"]+)""#).expect("valid regex"),
            Regex::new(r#"\("([A-Z0-9]{10})","([0-9a-f]{32})"\)"#).expect("valid regex"),
        ]
    });
    let hub = client
        .get_text(&format!("{BASE_URL}/bulletins-officiels"))
        .await?;
    let missing = |what: &str| SourceError::Invalid(format!("culture: no search {what}"));
    let script = script.find(&hub).ok_or_else(|| missing("script"))?;
    let index = index.captures(&hub).ok_or_else(|| missing("index"))?[1].to_string();
    let js = client
        .get_text(&format!("{BASE_URL}{}", script.as_str()))
        .await?;
    let key = key.captures(&js).ok_or_else(|| missing("key"))?;
    let (app, key) = (&key[1], &key[2]);
    let mut out = Vec::new();
    for page in 0.. {
        let url = format!(
            "https://{app}-dsn.algolia.net/1/indexes/{index}?query=&filters=content_type_id%3A248&hitsPerPage=1000&page={page}&x-algolia-application-id={app}&x-algolia-api-key={key}"
        );
        let found: SearchPage = serde_json::from_str(&client.get_text(&url).await?)
            .map_err(|e| SourceError::Invalid(format!("culture search: {e}")))?;
        out.extend(
            found
                .hits
                .into_iter()
                .filter_map(per_text_notice)
                .filter(|n| n.published.is_some_and(|d| d.year() >= since_year)),
        );
        if page + 1 >= found.nb_pages {
            break;
        }
    }
    Ok(out)
}

fn per_text_notice(h: Hit) -> Option<BulletinNotice> {
    let title = h
        .official_bulletin_name_value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let url = format!("{BASE_URL}{}", h.url);
    Some(BulletinNotice {
        site: BulletinSite::Culture,
        native_id: h.object_id,
        nor: h
            .official_bulletin_nor_value
            .as_deref()
            .and_then(super::nor),
        kind: title.split_whitespace().next()?.to_string(),
        signed: signed_in_title(&title),
        published: DateTime::from_timestamp(h.content_publication_date, 0).map(|d| d.date_naive()),
        notice_url: url.clone(),
        body_url: Some(url),
        body: None,
        legifrance_uid: None,
        doctrine: is_doctrine(&title),
        abrogated: false,
        title,
    })
}

async fn issues(client: &BulletinClient, since_year: i32) -> Result<Vec<BulletinNotice>> {
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [issue_link, pdf_link, released] = RE.get_or_init(|| {
        [
            Regex::new(&format!(
                r#"href="{BASE_URL}{ARCHIVE}/((?i:bulletin-officiel)-[^"]*?(\d{{4}}))""#
            ))
            .expect("valid regex"),
            Regex::new(r#"href="(/mc/content/download/[^"]+\.pdf[^"]*)""#).expect("valid regex"),
            Regex::new(r#"Parution le\s*<time datetime="(\d{4}-\d\d-\d\d)""#).expect("valid regex"),
        ]
    });
    let mut slugs = BTreeSet::new();
    for page in 1.. {
        let html = client
            .get_text(&format!("{BASE_URL}{ARCHIVE}?limit=60&page={page}"))
            .await?;
        let before = slugs.len();
        let mut older = false;
        for c in issue_link.captures_iter(&html) {
            let year: i32 = c[2].parse().unwrap_or(0);
            if year >= since_year {
                slugs.insert(c[1].to_string());
            } else {
                older = true;
            }
        }
        if older || slugs.len() == before {
            break;
        }
    }
    let mut out = Vec::new();
    for slug in slugs {
        let html = client
            .get_text(&format!("{BASE_URL}{ARCHIVE}/{slug}"))
            .await?;
        let Some(pdf) = pdf_link.captures(&html).map(|c| decode_entities(&c[1])) else {
            tracing::warn!(issue = %slug, "culture: issue without a pdf");
            continue;
        };
        let published = released
            .captures(&html)
            .and_then(|c| NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").ok());
        let pdf = format!("{BASE_URL}{pdf}");
        let Some(bytes) = client.get_bytes(&pdf).await? else {
            tracing::warn!(issue = %slug, "culture: issue pdf gone");
            continue;
        };
        let (layout, plain) = tokio::task::spawn_blocking(move || {
            Ok::<_, SourceError>((
                crate::pdf::pdftotext_layout_extract(&bytes)?,
                crate::pdf::pdftotext_extract(&bytes)?,
            ))
        })
        .await
        .map_err(|e| SourceError::Invalid(format!("pdftotext task: {e}")))??;
        let texts = cut_issue(&issue_id(&slug), &pdf, published, &layout, &plain);
        if texts.is_empty() {
            tracing::warn!(issue = %slug, "culture: issue cut to no text");
        }
        out.extend(texts);
    }
    Ok(out)
}

/// `bulletin-officiel-n-290-fevrier-2019` gives `290`, a special issue `hs4`.
fn issue_id(slug: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)(hors-serie-)?n-(\d+)").expect("valid regex"));
    re.captures(slug).map_or_else(
        || slug.to_lowercase(),
        |c| format!("{}{}", if c.get(1).is_some() { "hs" } else { "" }, &c[2]),
    )
}

struct TocEntry {
    title: String,
    page: u32,
}

/// The texts of one issue, each cut from its first printed page to the next
/// entry's, then trimmed to its own title and the next one.
pub fn cut_issue(
    issue: &str,
    pdf: &str,
    published: Option<NaiveDate>,
    layout: &str,
    plain: &str,
) -> Vec<BulletinNotice> {
    let (toc, toc_page) = table_of_contents(layout);
    let pages: Vec<&str> = plain.split('\u{c}').collect();
    let printed: Vec<Option<u32>> = pages
        .iter()
        .map(|p| p.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|l| l.and_then(|l| l.trim().parse().ok()))
        .collect();
    // A table or a landscape page prints its number mid-text.
    let index_of = |n: u32| {
        printed.iter().position(|p| *p == Some(n)).or_else(|| {
            let (k, p) = printed
                .iter()
                .enumerate()
                .filter_map(|(k, p)| p.filter(|&p| p < n).map(|p| (k, p)))
                .max_by_key(|&(_, p)| p)?;
            Some(k + (n - p) as usize).filter(|&i| i < pages.len())
        })
    };
    // An issue of 1997 numbers its pages one short of its summary, which sends
    // the first text onto the summary's own page.
    let shift = toc
        .first()
        .and_then(|e| index_of(e.page))
        .map_or(0, |f| (toc_page + 1).saturating_sub(f));
    let index_of = |n: u32| index_of(n).map(|i| (i + shift).min(pages.len() - 1));
    let mut out = Vec::new();
    for (k, e) in toc.iter().enumerate() {
        if e.title.starts_with("Bulletin d’abonnement")
            || e.title.starts_with("Bulletin d'abonnement")
        {
            continue;
        }
        let next = toc.get(k + 1);
        let Some(first) = index_of(e.page) else {
            tracing::warn!(issue, page = e.page, "culture: printed page not found");
            continue;
        };
        let last = next
            .and_then(|n| index_of(n.page))
            .filter(|&l| l >= first)
            .unwrap_or(pages.len() - 1);
        let mut text = pages[first..=last].join("\n");
        // A title the pages do not print as the summary does leaves no edge to cut on.
        let Some(start) = title_at(&text, &e.title) else {
            tracing::info!(issue, title = %e.title, "culture: summary entry not found in its pages");
            continue;
        };
        text.drain(..start);
        let skip = text.chars().next().map_or(0, char::len_utf8);
        if let Some(end) = next.and_then(|n| title_at(&text[skip..], &n.title)) {
            text.truncate(skip + end);
        }
        out.push(BulletinNotice {
            site: BulletinSite::Culture,
            native_id: format!("{issue}-{}", k + 1),
            nor: None,
            kind: e.title.split_whitespace().next().unwrap_or("").to_string(),
            signed: signed_in_title(&e.title),
            published,
            notice_url: format!("{pdf}#page={}", first + 1),
            body_url: None,
            body: Some(strip_running_heads(&text)),
            legifrance_uid: None,
            doctrine: is_doctrine(&e.title),
            abrogated: false,
            title: e.title.trim_end_matches('.').to_string(),
        });
    }
    out
}

/// Entries end with `Page N` after a run of spaces, or, in the issues before
/// 2015, open with it, and then come with the page the summary ends on; a title
/// runs over its lines until its final period, or a blank line.
fn table_of_contents(layout: &str) -> (Vec<TocEntry>, usize) {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [page_last, page_first] = RE.get_or_init(|| {
        [
            Regex::new(r"^(.*?)\s{2,}Page\s+(\d+)\s*$").expect("valid regex"),
            Regex::new(r"^Page\s+(\d+)\s{2,}(\S.*)$").expect("valid regex"),
        ]
    });
    let raw: Vec<&str> = layout.lines().collect();
    let lines: Vec<&str> = raw.iter().map(|l| l.trim()).collect();
    let Some(start) = lines.iter().position(|l| *l == "SOMMAIRE") else {
        return (Vec::new(), 0);
    };
    let window = start..lines.len().min(start + 3000);
    let Some(leading) = window.clone().find_map(|i| {
        if page_last.is_match(lines[i]) {
            Some(false)
        } else {
            page_first.is_match(lines[i]).then_some(true)
        }
    }) else {
        return (Vec::new(), 0);
    };
    let entry = |line: &str| -> Option<(String, u32)> {
        let (title, page) = if leading {
            page_first
                .captures(line)
                .map(|c| (c[2].to_string(), c[1].to_string()))?
        } else {
            page_last
                .captures(line)
                .map(|c| (c[1].to_string(), c[2].to_string()))?
        };
        Some((
            title.split_whitespace().collect::<Vec<_>>().join(" "),
            page.parse().unwrap_or(0),
        ))
    };
    let Some(end) = window.rev().find(|&i| entry(lines[i]).is_some()) else {
        return (Vec::new(), 0);
    };
    let toc_page = if leading {
        raw[..=end].iter().filter(|l| l.contains('\u{c}')).count()
    } else {
        0
    };
    // The last title's own lines follow its page.
    let end = if leading {
        (end + 8).min(lines.len() - 1)
    } else {
        end
    };
    let mut out: Vec<TocEntry> = Vec::new();
    let mut open = false;
    for &line in &lines[start..=end] {
        let parsed = entry(line);
        // A page set beside an open title's last word rather than its first.
        if let Some((tail, _)) = parsed.as_ref().filter(|(t, _)| {
            leading && open && t.ends_with('.') && t.split_whitespace().count() < 3
        }) {
            let e = out.last_mut().expect("open implies an entry");
            e.title.push(' ');
            e.title.push_str(tail);
            open = false;
        } else if let Some((title, page)) = parsed {
            open = !title.ends_with('.');
            out.push(TocEntry { title, page });
        } else if line.is_empty() {
            open &= !leading;
        } else if open
            && !line.chars().all(|c| c.is_ascii_digit())
            && !line.contains("Bulletin officiel")
        {
            let e = out.last_mut().expect("open implies an entry");
            e.title.push(' ');
            e.title
                .push_str(&line.split_whitespace().collect::<Vec<_>>().join(" "));
            open = !line.ends_with('.');
        }
    }
    (out, toc_page)
}

/// Where the longest run of the title's first words, six at least, starts in the
/// text, whatever the line breaks: texts printed side by side share their opening.
pub(super) fn title_at(text: &str, title: &str) -> Option<usize> {
    let words: Vec<String> = title.split_whitespace().map(regex::escape).collect();
    let find = |n: usize| {
        Regex::new(&words[..n].join(r"\s+"))
            .ok()?
            .find(text)
            .map(|m| m.start())
    };
    let (mut lo, mut hi) = (words.len().min(6), words.len());
    let mut at = find(lo)?;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        match find(mid) {
            Some(found) => (at, lo) = (found, mid),
            None => hi = mid - 1,
        }
    }
    Some(at)
}

/// Without the page numbers, the issue's running head and repeated blank lines.
fn strip_running_heads(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let head = !l.is_empty() && l.chars().all(|c| c.is_ascii_digit())
            || l.starts_with("Bulletin officiel") && l.len() < 30;
        if head || l.is_empty() && out.last().is_some_and(|p| p.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

fn signed_in_title(title: &str) -> Option<NaiveDate> {
    let head: String = title.chars().take(120).collect();
    head.split_once(" du ")
        .and_then(|(_, d)| french_long_date(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_is_cut_into_its_texts_by_the_table_of_contents() {
        let layout = "Bulletin officiel 290\n\n                SOMMAIRE\n\
            Décision du 1er février 2019 confiant l’intérim de l’emploi.        Page 1\n\
            Instruction n° 2019/002 du 22 février 2019 relative à l’égalité      Page 1\n\
            professionnelle entre les femmes et les hommes.\n\
            Arrêté du 27 février 2019 portant cessation de fonctions.           Page 3\n";
        let plain = "SOMMAIRE\n\nX\n\u{c}Bulletin officiel 290\n\nDécision du 1er février 2019 confiant\nl’intérim.\nInstruction n° 2019/002 du 22 février\n2019 relative à l’égalité professionnelle\nPremier alinéa.\n1\n\u{c}Bulletin officiel 290\n\nSecond alinéa.\n2\n\u{c}Arrêté du 27 février 2019 portant\ncessation.\n3\n";
        let notices = cut_issue("290", "https://x/bo.pdf", None, layout, plain);
        assert_eq!(notices.len(), 3);
        assert!(!notices[0].doctrine);
        assert_eq!(
            notices[2].title,
            "Arrêté du 27 février 2019 portant cessation de fonctions"
        );
        let n = &notices[1];
        assert!(n.doctrine);
        assert_eq!(n.text_uid(), "bo_culture_290-2");
        assert_eq!(
            n.title,
            "Instruction n° 2019/002 du 22 février 2019 relative à l’égalité professionnelle entre les femmes et les hommes"
        );
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2019, 2, 22));
        assert_eq!(n.notice_url, "https://x/bo.pdf#page=2");
        assert_eq!(
            n.body.as_deref(),
            Some("Instruction n° 2019/002 du 22 février\n2019 relative à l’égalité professionnelle\nPremier alinéa.\n\nSecond alinéa.")
        );
    }

    #[test]
    fn a_page_whose_number_is_not_its_last_line_is_counted_from_the_one_before() {
        let layout = "SOMMAIRE\n\
            Arrêté du 2 octobre 2019 portant délégation de signature.          Page 7\n\
            Note de gestion du 3 octobre 2019 relative aux primes.            Page 8\n";
        let plain = "SOMMAIRE\n\u{c}Arrêté du 2 octobre 2019 portant\ndélégation.\n7\n\u{c}Note de gestion du 3 octobre 2019\n8\nTableau.\nBulletin officiel 297\n";
        let notices = cut_issue("297", "https://x/bo.pdf", None, layout, plain);
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[1].notice_url, "https://x/bo.pdf#page=3");
        assert_eq!(
            notices[0].body.as_deref(),
            Some("Arrêté du 2 octobre 2019 portant\ndélégation.")
        );
    }

    #[test]
    fn an_old_summary_opens_its_entries_on_their_page() {
        let layout = "                SOMMAIRE\n\
            Page 1       Circulaire du 16 janvier 1997 relative aux\n\
            \x20            archives communales.\n\
            \n\
            \x20            Musées\n\
            Page 2       Décision du 10 avril 1997 relative au tarif du Grand\n\
            Page 2       Palais.\n\
            Page 3       Bulletin d’abonnement\n";
        let plain = "SOMMAIRE\n1\n\u{c}Circulaire du 16 janvier 1997 relative aux\narchives communales.\nTexte.\n2\n\u{c}Décision du 10 avril 1997 relative au tarif\ndu Grand Palais.\n3\n\u{c}Bulletin d’abonnement\n4\n";
        let notices = cut_issue("100", "https://x/bo.pdf", None, layout, plain);
        assert_eq!(notices.len(), 2);
        assert_eq!(
            notices[0].title,
            "Circulaire du 16 janvier 1997 relative aux archives communales"
        );
        assert_eq!(notices[0].notice_url, "https://x/bo.pdf#page=2");
        assert_eq!(
            notices[0].body.as_deref(),
            Some("Circulaire du 16 janvier 1997 relative aux\narchives communales.\nTexte.")
        );
        assert_eq!(
            notices[1].title,
            "Décision du 10 avril 1997 relative au tarif du Grand Palais"
        );
        assert_eq!(notices[1].text_uid(), "bo_culture_100-2");
    }

    #[test]
    fn texts_sharing_their_opening_are_told_apart_by_their_whole_title() {
        let layout = "SOMMAIRE\n\
            Arrêté du 18 juillet 2011 portant nomination du président de la commission Littérature.   Page 1\n\
            Arrêté du 18 juillet 2011 portant nomination du président de la commission Roman.         Page 1\n";
        let plain = "SOMMAIRE\n\u{c}Arrêté du 18 juillet 2011 portant nomination du\nprésident de la commission Roman.\nM. A.\nArrêté du 18 juillet 2011 portant nomination du\nprésident de la commission Littérature.\nMme B.\n1\n";
        let notices = cut_issue("200", "https://x/bo.pdf", None, layout, plain);
        assert_eq!(notices.len(), 2);
        assert_eq!(
            notices[0].body.as_deref(),
            Some("Arrêté du 18 juillet 2011 portant nomination du\nprésident de la commission Littérature.\nMme B.")
        );
        assert!(notices[1].body.as_deref().is_some_and(|b| b.starts_with(
            "Arrêté du 18 juillet 2011 portant nomination du\nprésident de la commission Roman."
        )));
    }

    #[test]
    fn a_special_issue_keeps_its_mark() {
        assert_eq!(
            issue_id("Bulletin-officiel-Hors-serie-n-4-janvier-2021"),
            "hs4"
        );
        assert_eq!(issue_id("bulletin-officiel-n-366-janvier-2026"), "366");
    }
}
