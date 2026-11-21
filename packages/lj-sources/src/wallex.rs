//! Wallex, the Walloon law portal: its search over the Region's acts, its update feed, an act's page.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITE: &str = "https://wallex.wallonie.be";
const SITE_HOME: &str = "/sites/wallex/home";
const WALLOON_REGION: &str = "65aacf45-4abe-4dbb-9d8f-e072d734a287";
/// The search answers at most this many acts, a range that fills it is split.
const CAP: usize = 100;
const FIRST_YEAR: i32 = 1970;

#[derive(Deserialize)]
struct Search {
    acts: Found,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    total_count: usize,
    results: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    path: String,
}

#[derive(Deserialize)]
struct Updates {
    results: Vec<Update>,
}

#[derive(Deserialize)]
struct Update {
    date: i64,
    path: String,
}

/// An act's Wallex number from a content path: `/sites/wallex/contents/acts/135/135745/1`.
fn act_id(path: &str) -> Option<u64> {
    let mut parts = path.split('/').skip_while(|p| *p != "acts").skip(2);
    parts.next()?.parse().ok()
}

pub struct WallexClient {
    paced: PacedClient,
}

impl WallexClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "wallex")?,
        })
    }

    /// The act's page, which the site sends on to its ELI address.
    pub fn page_url(id: u64) -> String {
        format!("{SITE}/contents/acts/{}/{id}.html", id / 1000)
    }

    pub fn url(path: &str) -> String {
        format!("{SITE}{path}")
    }

    async fn post(&self, path: &str, form: &str) -> Result<Vec<u8>> {
        let url = format!("{SITE}{path}");
        self.paced
            .send(|| {
                self.paced
                    .http
                    .post(&url)
                    .header("X-Requested-With", "XMLHttpRequest")
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(form.to_string())
            })
            .await
    }

    async fn promulgated(&self, from: NaiveDate, until: NaiveDate) -> Result<Found> {
        let form = format!(
            "searchKeyword=&titleOnly=false&contentOnly=false&exactMatch=false&lawState=\
             &promulgationDate=range&promulgationDateFrom={}&promulgationDateUntil={}\
             &publicationDate=&includeChangingText=false&referenceNumber=&legalOrder={WALLOON_REGION}\
             &author=&nature=&source=&theme=&djas=l",
            from.format("%d%%2F%m%%2F%Y"),
            until.format("%d%%2F%m%%2F%Y"),
        );
        let path = format!("{SITE_HOME}/search/content-area/rercherce-acte.wallexSearch.do");
        let body = self.post(&path, &form).await?;
        let found: Search = serde_json::from_slice(&body)
            .map_err(|e| SourceError::Invalid(format!("wallex search {from}..{until}: {e}")))?;
        Ok(found.acts)
    }

    /// Every act of the Region by promulgation date, a range the search caps halved until whole.
    pub async fn listing(&self, today: NaiveDate) -> Result<BTreeSet<u64>> {
        let mut ids = BTreeSet::new();
        let mut ranges: Vec<(NaiveDate, NaiveDate)> = (FIRST_YEAR..=today.year())
            .step_by(10)
            .filter_map(|y| {
                Some((
                    NaiveDate::from_ymd_opt(y, 1, 1)?,
                    NaiveDate::from_ymd_opt((y + 9).min(today.year()), 12, 31)?,
                ))
            })
            .collect();
        ranges.reverse();
        while let Some((from, until)) = ranges.pop() {
            let found = self.promulgated(from, until).await?;
            if found.total_count >= CAP && from < until {
                let mid = from + (until - from) / 2;
                ranges.push((mid.succ_opt().expect("a day after"), until));
                ranges.push((from, mid));
                continue;
            }
            if found.total_count >= CAP {
                return Err(SourceError::Invalid(format!(
                    "wallex promulgated {CAP} acts or more on {from}"
                )));
            }
            ids.extend(found.results.iter().filter_map(|h| act_id(&h.path)));
        }
        Ok(ids)
    }

    /// The acts the feed of last updates names since `since`, page after page, newest first.
    pub async fn changed_since(&self, since: DateTime<Utc>) -> Result<BTreeSet<u64>> {
        let path = format!(
            "/cms/render/live/fr{SITE_HOME}/dernieres-mise-a-jour/content-area/container/container/wallex---last-updates.wallexSearchLastUpdate.do"
        );
        let mut ids = BTreeSet::new();
        for page in 0.. {
            let body = self.post(&path, &format!("limit=25&offset={page}")).await?;
            let updates: Updates = serde_json::from_slice(&body)
                .map_err(|e| SourceError::Invalid(format!("wallex updates page {page}: {e}")))?;
            let mut older = updates.results.is_empty();
            for u in &updates.results {
                if u.date < since.timestamp_millis() {
                    older = true;
                } else {
                    ids.extend(act_id(&u.path));
                }
            }
            if older {
                break;
            }
        }
        Ok(ids)
    }

    pub async fn page(&self, url: &str) -> Result<String> {
        self.paced.text(url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_act_is_numbered_by_its_content_path() {
        assert_eq!(
            act_id("/sites/wallex/contents/acts/135/135745/1"),
            Some(135745)
        );
        assert_eq!(act_id("/fr/sites/wallex/contents/acts/2/2661"), Some(2661));
        assert_eq!(act_id("/sites/wallex/contents/djas/5/5403"), None);
        assert_eq!(
            WallexClient::page_url(135745),
            "https://wallex.wallonie.be/contents/acts/135/135745.html"
        );
    }
}
