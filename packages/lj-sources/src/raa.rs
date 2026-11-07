//! The prefectures' recueils des actes administratifs: listed through Attrap's
//! search, each PDF read from its prefecture's site.

use std::time::Duration;

use chrono::NaiveDate;
use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::hal::USER_AGENT;

const ATTRAP_SEARCH: &str = "https://attrap.fr/api/v1/search";
const PAGE_SIZE: usize = 50;
/// OpenSearch serves no hit past this rank, so a window must hold fewer.
const WINDOW_CAP: usize = 10_000;

/// A recueil as Attrap lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct Listed {
    pub administration: String,
    pub name: String,
    pub date: Option<NaiveDate>,
    pub url: String,
}

#[derive(Deserialize)]
struct SearchPage {
    total: usize,
    elements: Vec<Listed>,
}

pub enum Fetched {
    Pdf(Vec<u8>),
    /// 404 or 410: the prefecture no longer serves the file.
    Gone,
    /// 403, 429 or 503 from a host outside the gateway.
    Refused(u16),
}

/// Hosts read through the page gateway.
const GATEWAY_HOSTS: &[&str] = &["www.prefecturedepolice.interieur.gouv.fr"];

pub struct RaaClient {
    http: reqwest::Client,
    /// The gateway sits beside the proxy, never behind it.
    local: reqwest::Client,
    page_gateway: Option<String>,
}

impl RaaClient {
    /// Every request leaves through `proxy`, the gated hosts' through `page_gateway`.
    pub fn new(proxy: &str, page_gateway: Option<String>) -> Result<Self> {
        let client = || {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(300))
        };
        Ok(Self {
            http: client().proxy(reqwest::Proxy::all(proxy)?).build()?,
            local: client().no_proxy().build()?,
            page_gateway,
        })
    }

    /// The recueils Attrap dates from `from` to `to`, both included.
    pub async fn list(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Listed>> {
        let (from, to) = (from.to_string(), to.to_string());
        let mut listed = Vec::new();
        for page in 1.. {
            let response = self
                .http
                .get(ATTRAP_SEARCH)
                .query(&[
                    ("s", "*"),
                    ("start_date", &from),
                    ("end_date", &to),
                    ("sort", "asc"),
                    ("size", &PAGE_SIZE.to_string()),
                    ("page", &page.to_string()),
                ])
                .send()
                .await?
                .error_for_status()?;
            let body: SearchPage = response.json().await?;
            if body.total >= WINDOW_CAP {
                return Err(SourceError::Invalid(format!(
                    "attrap: {} hits from {from} to {to}, past the {WINDOW_CAP} it serves",
                    body.total
                )));
            }
            let done = body.elements.is_empty() || page * PAGE_SIZE >= body.total;
            listed.extend(body.elements);
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Ok(listed)
    }

    pub async fn fetch(&self, url: &str) -> Result<Fetched> {
        let gated = reqwest::Url::parse(url)
            .map_err(|e| SourceError::Invalid(format!("raa {url}: {e}")))?
            .host_str()
            .is_some_and(|h| GATEWAY_HOSTS.contains(&h));
        let response = if gated {
            let Some(gateway) = &self.page_gateway else {
                return Err(SourceError::Invalid(format!(
                    "LIBREJUSTICE_PAGE_GATEWAY is empty: {url} is read through the page gateway"
                )));
            };
            self.local
                .get(format!("{gateway}/fetch"))
                .query(&[("url", url)])
                .send()
                .await?
        } else {
            self.http.get(url).send().await?
        };
        let status = response.status().as_u16();
        match status {
            200 => {
                let bytes = response.bytes().await?.to_vec();
                if !bytes.starts_with(b"%PDF") {
                    return Err(SourceError::Invalid(format!("raa {url}: not a PDF")));
                }
                Ok(Fetched::Pdf(bytes))
            }
            404 | 410 => Ok(Fetched::Gone),
            403 | 429 | 503 if !gated => Ok(Fetched::Refused(status)),
            _ => Err(SourceError::Invalid(format!("raa {url}: status {status}"))),
        }
    }
}

/// The département where the publisher sits: its own for a prefecture, the
/// regional capital's for a regional prefecture.
pub fn seat_department(administration: &str) -> Option<&'static str> {
    Some(match administration {
        "ppparis" | "prefidf" => "75",
        "prefara" => "69",
        "prefbfc" => "21",
        "prefbretagne" => "35",
        "prefcvl" => "45",
        "prefcorse" => "2A",
        "prefge" => "67",
        "prefhdf" => "59",
        "prefnormandie" => "76",
        "prefna" => "33",
        "prefoccitanie" => "31",
        "prefpl" => "44",
        "prefpaca" => "13",
        "hcnc" => "988",
        "hcpf" => "987",
        "pref2a" => "2A",
        "pref2b" => "2B",
        other => {
            let code = other.strip_prefix("pref")?;
            DEPARTMENTS.iter().find(|d| **d == code)?
        }
    })
}

const DEPARTMENTS: &[&str] = &[
    "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15", "16",
    "17", "18", "19", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32", "33",
    "34", "35", "36", "37", "38", "39", "40", "41", "42", "43", "44", "45", "46", "47", "48", "49",
    "50", "51", "52", "53", "54", "55", "56", "57", "58", "59", "60", "61", "62", "63", "64", "65",
    "66", "67", "68", "69", "70", "71", "72", "73", "74", "75", "76", "77", "78", "79", "80", "81",
    "82", "83", "84", "85", "86", "87", "88", "89", "90", "91", "92", "93", "94", "95", "971",
    "972", "973", "974", "976",
];
