//! Justel, the Moniteur's consolidated legislation: a document's current and archived pages, the
//! acts a search lists, and the feed of recent consolidations.

use chrono::NaiveDate;
use lj_parse::{JustelConsolidated, JustelList, JustelListed};

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const HOST: &str = "www.ejustice.just.fgov.be";

/// Windows-1252 from 0x80 to 0x9F, where it departs from the Latin-1 the pages declare.
const HIGH: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

fn windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9f => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

pub struct JustelClient {
    paced: PacedClient,
}

impl JustelClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "justel")?,
        })
    }

    /// The current page of the document published as `numac`, numbered `cn` when it is known, or
    /// its `archive`th archived one.
    pub fn url(numac: &str, cn: Option<&str>, archive: Option<usize>) -> String {
        let archive = archive.map_or(String::new(), |n| format!("&arch={n:03}"));
        let (cn, view) = match cn {
            Some(cn) => (cn.to_string(), cn.to_string()),
            None => (String::new(), numac.to_lowercase()),
        };
        format!(
            "https://{HOST}/cgi_loi/article.pl?language=fr{archive}&lg_txt=f&type=&sort=\
             &numac_search={numac}&cn_search={cn}&caller=eli&&view_numac={view}fr"
        )
    }

    /// The acts `issuer` adopted between the bounds, or published between them when `published`,
    /// all of their pages; `None` when Justel refuses a search naming more than it lists.
    pub async fn listed(
        &self,
        issuer: &str,
        (from, to): (NaiveDate, NaiveDate),
        published: bool,
    ) -> Result<Option<Vec<JustelListed>>> {
        let bounds = match published {
            true => format!("&pdd={from}&pdf={to}"),
            false => format!("&ddd={from}&ddf={to}"),
        };
        let issuer: String = url::form_urlencoded::byte_serialize(issuer.as_bytes()).collect();
        let mut acts = Vec::new();
        let mut page = 1;
        loop {
            let url = format!(
                "https://{HOST}/cgi_loi/list.pl?language=fr&sum_date=&page={page}\
                 &trier=promulgation{bounds}&fr=f&so={issuer}&choix1=ET&choix2=ET"
            );
            let list: JustelList = lj_parse::justel_list(&self.page(&url).await?);
            if list.capped {
                return Ok(None);
            }
            if list.acts.is_empty() && page <= list.pages {
                return Err(SourceError::Invalid(format!(
                    "{url}: page {page} lists no act"
                )));
            }
            acts.extend(list.acts);
            if page >= list.pages {
                return Ok(Some(acts));
            }
            page += 1;
        }
    }

    /// The acts Justel consolidated again over its last working days, newest first.
    pub async fn consolidated(&self) -> Result<Vec<JustelConsolidated>> {
        let url = format!(
            "https://{HOST}/cgi_loi/summary.pl?language=fr&type=cons&sort=date_upd&view_numac="
        );
        Ok(lj_parse::justel_consolidated(&self.page(&url).await?))
    }

    pub async fn page(&self, url: &str) -> Result<String> {
        let body = self.paced.send(|| self.paced.http.get(url)).await?;
        Ok(windows_1252(&body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_decodes_as_windows_1252() {
        assert_eq!(
            windows_1252(b"l\x92article \xe9t\xe9 \x80"),
            "l\u{2019}article \u{e9}t\u{e9} \u{20ac}"
        );
    }
}
