//! The bulletins printed one PDF per issue, each text under its title with
//! its NOR.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::{french_long_date, is_doctrine, BulletinNotice, BulletinSite};

struct Head {
    nor: String,
    at: usize,
    page: usize,
    page_start: usize,
    /// Where the text's title starts, and the title.
    title: Option<(usize, String)>,
}

/// The texts of one issue. Every text states its NOR under its title and runs
/// to the next text's title, or to the page that title opens; a text printed
/// in two parts states it twice.
pub fn cut_issue(
    site: BulletinSite,
    pdf: &str,
    plain: &str,
    published: Option<NaiveDate>,
) -> Vec<BulletinNotice> {
    let plain = &plain.replace(
        |c: char| c.is_control() && !matches!(c, '\n' | '\u{c}'),
        " ",
    );
    let running = running_heads(plain);
    let heads = heads(plain);
    let mut out: Vec<BulletinNotice> = Vec::new();
    for (k, h) in heads.iter().enumerate() {
        let Some((start, title)) = &h.title else {
            continue;
        };
        let end = match heads.get(k + 1) {
            None => plain.len(),
            Some(next)
                if next.page_start > h.page_start
                    && next
                        .title
                        .as_ref()
                        .is_none_or(|(s, _)| s - next.page_start < 1500) =>
            {
                next.page_start
            }
            Some(Head {
                title: Some((s, _)),
                ..
            }) if *s > h.at => *s,
            Some(next) => next.at,
        };
        let body = strip_running_heads(&plain[*start..end], &running);
        if let Some(n) = out.iter_mut().find(|n| n.native_id == h.nor) {
            let first = n.body.as_mut().expect("a cut text carries its body");
            first.push_str("\n\n");
            first.push_str(&body);
            continue;
        }
        out.push(BulletinNotice {
            site,
            native_id: h.nor.clone(),
            nor: Some(h.nor.clone()),
            kind: title.split_whitespace().next().unwrap_or("").to_string(),
            signed: signed_in_title(title),
            published,
            notice_url: format!("{pdf}#page={}", h.page + 1),
            body_url: None,
            body: Some(body),
            legifrance_uid: None,
            doctrine: is_doctrine(title),
            abrogated: false,
            title: title.clone(),
        });
    }
    out
}

/// The title is the first line of the paragraph above the NOR that opens with
/// a capitalised type, else the last such line of the page, else a dated title
/// closing the page before.
fn heads(plain: &str) -> Vec<Head> {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [anchor, kind, verb, stop] = RE.get_or_init(|| {
        [
            Regex::new(r"(?m)^NOR\s*:\s*([A-Z]{4}\d{7}[A-Z])").expect("valid regex"),
            Regex::new(r"^(?i:instruction|circulaire|note|directive|lignes directrices|arr[êe]t[ée]|d[ée]cision|convention|avis|liste|d[ée]lib[ée]ration|d[ée]cret|rectificatif|charte|protocole|avenant|recommandation|cahier des charges|tableau|annexe|lettre|saisine)\b")
                .expect("valid regex"),
            Regex::new(r"^(?i:arr[êe]te)\s*:").expect("valid regex"),
            Regex::new(r"^(?:(?:La|Le|Les) (?:Premier )?ministres?\b.*|Références?\b.*|à)$")
                .expect("valid regex"),
        ]
    });
    let opens =
        |l: &str| l.starts_with(char::is_uppercase) && kind.is_match(l) && !verb.is_match(l);
    let page_of = |at: usize| plain[..at].rfind('\u{c}').map_or(0, |i| i + 1);
    let lines_of = |from: usize, to: usize| {
        let mut lines = Vec::new();
        let mut offset = from;
        for line in plain[from..to].split('\n') {
            lines.push((offset, line.trim()));
            offset += line.len() + 1;
        }
        lines
    };
    let title_from = |lines: &[(usize, &str)], k: usize| {
        let words: Vec<&str> = lines[k..]
            .iter()
            .map(|(_, l)| *l)
            .take_while(|l| !l.is_empty() && !stop.is_match(l))
            .collect();
        (lines[k].0, clean_title(&words.join(" ")))
    };
    anchor
        .captures_iter(plain)
        .map(|c| {
            let at = c.get(0).expect("whole match").start();
            let page_start = page_of(at);
            let mut lines = lines_of(page_start, at);
            lines.pop();
            let block = lines
                .iter()
                .rposition(|(_, l)| l.is_empty())
                .map_or(0, |b| b + 1);
            let on_page = (block..lines.len())
                .find(|&i| opens(lines[i].1))
                .or_else(|| lines.iter().rposition(|(_, l)| opens(l)));
            let (title, page_start) = match on_page {
                Some(k) => (Some(title_from(&lines, k)), page_start),
                None if page_start > 0 => {
                    let before = page_of(page_start - 1);
                    let lines = lines_of(before, page_start - 1);
                    let title = lines
                        .iter()
                        .rposition(|(_, l)| opens(l))
                        .map(|k| title_from(&lines, k))
                        .filter(|(_, t)| signed_in_title(t).is_some());
                    let start = if title.is_some() { before } else { page_start };
                    (title, start)
                }
                None => (None, page_start),
            };
            Head {
                nor: c[1].to_string(),
                at,
                page: plain[..page_start].matches('\u{c}').count(),
                page_start,
                title,
            }
        })
        .collect()
}

/// « INSTRUCTION INTERMINISTÉRIELLE N° … » as « Instruction interministérielle
/// n° … ».
fn clean_title(title: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let number = RE.get_or_init(|| Regex::new(r"\b[Nn][oº°]\s").expect("valid regex"));
    let title = number.replacen(title, 1, "n° ");
    let mut words: Vec<String> = title.split_whitespace().map(str::to_string).collect();
    let shouting = words
        .iter()
        .take_while(|w| {
            w.chars().any(char::is_alphabetic)
                && !w
                    .chars()
                    .any(|c| c.is_lowercase() || c.is_ascii_digit() || c == '/')
        })
        .count();
    for (i, w) in words.iter_mut().take(shouting).enumerate() {
        let lower = w.to_lowercase();
        *w = if i == 0 {
            let mut chars = lower.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        } else {
            lower
        };
    }
    words.join(" ").trim_end_matches('.').to_string()
}

/// The lines on more than half of the issue's pages.
pub(super) fn running_heads(plain: &str) -> BTreeSet<&str> {
    let pages: Vec<&str> = plain.split('\u{c}').collect();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for page in &pages {
        let lines: BTreeSet<&str> = page
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        for line in lines {
            *seen.entry(line).or_insert(0) += 1;
        }
    }
    seen.into_iter()
        .filter(|&(_, n)| n * 2 > pages.len())
        .map(|(line, _)| line)
        .collect()
}

/// Without the issue's running heads, the page numbers and repeated blank lines.
pub(super) fn strip_running_heads(text: &str, running: &BTreeSet<&str>) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let head = RE.get_or_init(|| {
        Regex::new(r"^(?:(?:BO|Bulletin officiel)\b.*\bn[oº°]\s?\d{4}/\d+\b.* du .*|.* INTÉRIEUR \d{4}-\d+ \S PAGE \d+|Page (?:\| )?\d+|\d+ ?/ ?\d+)$")
            .expect("valid regex")
    });
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if head.is_match(l)
            || running.contains(l)
            || !l.is_empty() && l.chars().all(|c| c.is_ascii_digit())
            || l.is_empty() && out.last().is_none_or(|p| p.trim().is_empty())
        {
            continue;
        }
        out.push(line.trim_start_matches('\u{c}'));
    }
    out.join("\n").trim().to_string()
}

/// « Instruction n° DGOS/P2/2025/142 du 21 octobre 2025 relative à … ».
pub(super) fn signed_in_title(title: &str) -> Option<NaiveDate> {
    let head: String = title.chars().take(120).collect();
    head.split_once(" du ")
        .and_then(|(_, after)| french_long_date(after))
        .or_else(|| {
            // A heading naming many services puts the text's date past the head.
            title.match_indices(" du ").find_map(|(i, _)| {
                french_long_date(&title[i + 4..].chars().take(20).collect::<String>())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_past_a_long_heading_is_read_where_it_follows_du() {
        assert_eq!(
            signed_in_title("Circulaire interministérielle DGAS/MAS/1C/Haut commissaire aux solidarités actives contre la pauvreté/DGCL/DIIESES/DGEFP/DREES n° 2007-409 du 25 octobre 2007 relative à la loi du 21 août 2007"),
            NaiveDate::from_ymd_opt(2007, 10, 25)
        );
        assert_eq!(
            signed_in_title("Note d'information de la direction générale de la cohésion sociale et de la direction de la sécurité sociale n° 12 relative à l'arrêté du 3 mai 2002"),
            NaiveDate::from_ymd_opt(2002, 5, 3)
        );
    }

    #[test]
    fn an_interior_issue_is_cut_into_its_texts_without_their_page_feet() {
        let plain = [
            "Décision n° 9402 du 17 octobre 2018 portant promotion\nNOR : INTJ1830001S\nArticle 1er\n15 avril 2019. \u{2013} INTÉRIEUR 2019-04 \u{2013} PAGE 1",
            "Instruction n° 9469 GEND/DPMGN/SDGP du 5 février 2019 relative à la prime\nNOR : INTJ1903807J\nRéférences :\n15 avril 2019. \u{2013} INTÉRIEUR 2019-04 \u{2013} PAGE 2",
            "Suite.\n15 avril 2019. \u{2013} INTÉRIEUR 2019-04 \u{2013} PAGE 3",
        ]
        .join("\n\u{c}");
        let notices = cut_issue(
            BulletinSite::Interieur,
            "https://x/bomi.pdf",
            &plain,
            NaiveDate::from_ymd_opt(2019, 4, 15),
        );
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0].text_uid(), "bo_interieur_INTJ1830001S");
        assert!(!notices[0].doctrine);
        let n = &notices[1];
        assert!(n.doctrine);
        assert_eq!(n.text_uid(), "bo_interieur_INTJ1903807J");
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2019, 2, 5));
        assert_eq!(n.notice_url, "https://x/bomi.pdf#page=2");
        assert_eq!(
            n.body.as_deref(),
            Some("Instruction n° 9469 GEND/DPMGN/SDGP du 5 février 2019 relative à la prime\nNOR : INTJ1903807J\nRéférences :\nSuite.")
        );
    }
}
