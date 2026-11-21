//! The higher education ministry's weekly bulletin, cut from each issue's PDF.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use super::issue::{running_heads, strip_running_heads};
use super::{
    french_date, french_long, french_long_date, is_doctrine, nor, BulletinClient, BulletinNotice,
    BulletinSite,
};
use crate::error::{Result, SourceError};
use crate::html_strip::{decode_entities, strip_html};

pub const BASE_URL: &str = "https://www.enseignementsup-recherche.gouv.fr";
const SEARCH: &str = "/fr/bulletin-officiel";

/// The search pages past the last one repeat it.
pub(super) async fn list(client: &BulletinClient, since: NaiveDate) -> Result<Vec<BulletinNotice>> {
    let today = chrono::Utc::now().date_naive();
    let mut issues = BTreeSet::new();
    for page in 0.. {
        let html = client
            .get_text(&format!(
                "{BASE_URL}{SEARCH}?field_publication_date_value%5Bmin%5D={since}\
                 &field_publication_date_value%5Bmax%5D={today}&page={page}"
            ))
            .await?;
        let before = issues.len();
        issues.extend(issue_links(&html));
        if issues.len() == before {
            break;
        }
    }
    let mut found = BTreeMap::new();
    // Three issue pages failing in a row is the gateway gone, not one issue: the run ends.
    let mut failed_in_a_row = 0;
    for url in issues {
        let html = match client.get_text(&url).await {
            Ok(html) => html,
            Err(e) if failed_in_a_row < 2 => {
                failed_in_a_row += 1;
                tracing::warn!(issue = %url, error = %e, "esr: issue page failed, its texts left to a later run");
                continue;
            }
            Err(e) => return Err(e),
        };
        failed_in_a_row = 0;
        let issue = parse_issue(&html);
        if issue.texts.is_empty() {
            tracing::warn!(issue = %url, "esr: issue page lists no text");
        }
        let mut cut: BTreeMap<String, (String, String)> = BTreeMap::new();
        if let Some(pdf) = &issue.pdf {
            let bytes = match client.frozen_issue(BulletinSite::Esr, pdf).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(issue = %url, error = %e, "esr: issue file failed, its texts left to a later run");
                    continue;
                }
            };
            if let Some(bytes) = bytes {
                let plain =
                    tokio::task::spawn_blocking(move || crate::pdf::pdftotext_extract(&bytes))
                        .await
                        .map_err(|e| SourceError::Invalid(format!("pdftotext task: {e}")))??;
                cut = cut_by_nor(&plain)
                    .into_iter()
                    .map(|(nor, (page, body))| (nor, (format!("{pdf}#page={page}"), body)))
                    .collect();
            }
        }
        for mut n in issue.texts {
            if let Some((url, body)) = n.nor.as_ref().and_then(|nor| cut.remove(nor)) {
                n.body = Some(body);
                n.notice_url = url;
            }
            found.entry(n.native_id.clone()).or_insert(n);
        }
    }
    Ok(found.into_values().collect())
}

/// The issues a search page lists, under the current scheme or the archive's.
fn issue_links(html: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let link = RE.get_or_init(|| {
        Regex::new(
            r#"href="(/fr/bo/\d{4}/[A-Za-z0-9-]+|/fr/pid20536/bulletin_officiel\.html\?[^"]+)""#,
        )
        .expect("valid regex")
    });
    link.captures_iter(html)
        .map(|c| format!("{BASE_URL}{}", decode_entities(&c[1])))
        .collect()
}

pub struct Issue {
    pub published: Option<NaiveDate>,
    pub pdf: Option<String>,
    pub texts: Vec<BulletinNotice>,
}

/// Each text's tile follows the heading of the rubric it falls under.
pub fn parse_issue(html: &str) -> Issue {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    let [heading, pdf, item, signed] = RE.get_or_init(|| {
        [
            Regex::new(r"<h1>\s*([^<]+?)\s*</h1>").expect("valid regex"),
            Regex::new(r#"class="fr-download__link" href="([^"]+\.pdf)""#).expect("valid regex"),
            Regex::new(
                r#"(?s)<h3 class="fr-h4">(.*?)</h3>|<a href="(/fr/bo/[^"]+)" hreflang="fr">(.*?)</a>\s*</h3>\s*<p class="fr-tile__desc">(.*?)</p>"#,
            )
            .expect("valid regex"),
            Regex::new(r"\bdu (\d{1,2}-\d{1,2}-\d{4})").expect("valid regex"),
        ]
    });
    let published = heading
        .captures(html)
        .and_then(|c| french_long_date(&clean(&c[1])));
    let pdf = pdf.captures(html).map(|c| format!("{BASE_URL}{}", &c[1]));
    let mut rubric = String::new();
    let mut texts = Vec::new();
    for c in item.captures_iter(html) {
        if let Some(r) = c.get(1) {
            rubric = clean(r.as_str());
            continue;
        }
        let path = &c[2];
        let native_id = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".htm")
            .to_string();
        let heading = clean(&c[3]);
        let desc = clean(&c[4]);
        let desc = desc
            .split_once("(NOR")
            .map_or(desc.as_str(), |(head, _)| head)
            .trim();
        let date = signed.captures(desc);
        let on = date.as_ref().and_then(|d| french_date(&d[1]));
        let head = match (&date, on) {
            (Some(d), Some(on)) => desc.replace(&d[1], &french_long(on)),
            _ => desc.to_string(),
        };
        let kind = desc
            .split([' ', '-'])
            .take_while(|w| !matches!(*w, "n°" | "du" | ""))
            .collect::<Vec<_>>()
            .join(" ");
        let subject = if rubric.is_empty() || rubric == heading {
            heading
        } else {
            format!("{rubric} : {heading}")
        };
        texts.push(BulletinNotice {
            site: BulletinSite::Esr,
            nor: nor(&native_id),
            notice_url: format!("{BASE_URL}{path}"),
            title: format!("{head}, {subject}"),
            signed: on.filter(|_| !desc.contains("J.O.")),
            published,
            body_url: None,
            body: None,
            legifrance_uid: None,
            doctrine: is_doctrine(&kind),
            abrogated: false,
            kind,
            native_id,
        });
    }
    Issue {
        published,
        pdf,
        texts,
    }
}

/// Each text opens with its rubric and heading, its NOR on a line of its own under them and its
/// type after: a text runs from that block to the next one. By NOR, its page and its body.
pub fn cut_by_nor(plain: &str) -> BTreeMap<String, (usize, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let anchor = RE.get_or_init(|| {
        Regex::new(r"(?m)^NOR\s*:\s*([A-Z]{4}\d{7}[A-Z])\s*$").expect("valid regex")
    });
    let starts: Vec<(String, usize)> = anchor
        .captures_iter(plain)
        .map(|c| {
            let at = c.get(0).expect("whole match").start();
            let block = plain[..at].rfind("\n\n").map_or(0, |b| b + 2);
            (c[1].to_string(), block)
        })
        .collect();
    let running = running_heads(plain);
    let mut out = BTreeMap::new();
    for (k, (nor, start)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(plain.len(), |(_, s)| *s);
        let page = plain[..*start].matches('\u{c}').count() + 1;
        let body = strip_running_heads(&plain[*start..end], &running);
        out.entry(nor.clone()).or_insert((page, body));
    }
    out
}

fn clean(s: &str) -> String {
    decode_entities(&strip_html(s))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_page_gives_each_text_its_rubric_type_date_and_nor() {
        let html = r#"<h1>Bulletin officiel n°2 du 12 janvier 2017</h1>
<a class="fr-download__link" href="/sites/default/files/imported_files/documents/BO_ESR_2_2-12-2017_694928.pdf" title="x">
<h3 class="fr-h4">Concours</h3>
    <div class="views-row"><article about="/fr/bo/17/Hebdo2/MENE1635861C.htm">
        <h3 class="fr-tile__title">
    <a href="/fr/bo/17/Hebdo2/MENE1635861C.htm" hreflang="fr">Trophée civisme et défense - édition 2016-2017</a>
        </h3>
                  <p class="fr-tile__desc">
    Circulaire
    n° 2016-206 du 6-1-2017
    (NOR : MENE1635861C)
</p>
<h3 class="fr-h4">Concours</h3>
        <h3 class="fr-tile__title">
    <a href="/fr/bo/17/Hebdo2/MENI1636337V.htm" hreflang="fr">Recrutement d&#039;inspecteurs généraux</a>
        </h3>
                  <p class="fr-tile__desc">
    Avis - J.O. du 23-12-2016
    (NOR : MENI1636337V)
</p>"#;
        let issue = parse_issue(html);
        assert_eq!(issue.published, NaiveDate::from_ymd_opt(2017, 1, 12));
        assert_eq!(
            issue.pdf.as_deref(),
            Some("https://www.enseignementsup-recherche.gouv.fr/sites/default/files/imported_files/documents/BO_ESR_2_2-12-2017_694928.pdf")
        );
        assert_eq!(issue.texts.len(), 2);
        let n = &issue.texts[0];
        assert_eq!(n.text_uid(), "bo_esr_MENE1635861C");
        assert_eq!(n.nor.as_deref(), Some("MENE1635861C"));
        assert_eq!(n.kind, "Circulaire");
        assert_eq!(n.signed, NaiveDate::from_ymd_opt(2017, 1, 6));
        assert_eq!(
            n.title,
            "Circulaire n° 2016-206 du 6 janvier 2017, Concours : Trophée civisme et défense - édition 2016-2017"
        );
        assert_eq!(n.nature(), Some("CIRCULAIRE"));
        let a = &issue.texts[1];
        assert_eq!(a.kind, "Avis");
        assert_eq!(a.signed, None);
        assert_eq!(
            a.title,
            "Avis - J.O. du 23 décembre 2016, Concours : Recrutement d'inspecteurs généraux"
        );
    }

    #[test]
    fn a_text_runs_from_its_nor_page_to_the_next_text() {
        let plain = "esr.gouv.fr\nBO n° 41\n1\n\nSommaire\nListe JO du 4-10-2026 - NOR : CTNR2625599K\n\u{c}\
esr.gouv.fr\nBO n° 41\n2\n\nOrganisation générale\nVocabulaire des batteries\nNOR : CTNR2625599K\nListe - JO du 4-10-2026\n\nI. Termes.\n\u{c}\
esr.gouv.fr\nBO n° 41\n3\n\nII. Fin des termes.\n\nTitres et diplômes\nAccréditation de Rouen\nNOR : ESRS2626955A\nArrêté du 9-10-2026\n\nArticle 1 - Accréditée.\n";
        let cut = cut_by_nor(plain);
        assert_eq!(cut.len(), 2);
        let (page, body) = &cut["CTNR2625599K"];
        assert_eq!(*page, 2);
        assert!(body
            .starts_with("Organisation générale\nVocabulaire des batteries\nNOR : CTNR2625599K"));
        assert!(body.ends_with("II. Fin des termes."));
        let (page, body) = &cut["ESRS2626955A"];
        assert_eq!(*page, 3);
        assert!(body.starts_with("Titres et diplômes\nAccréditation de Rouen"));
        assert!(body.ends_with("Article 1 - Accréditée."));
    }
}
