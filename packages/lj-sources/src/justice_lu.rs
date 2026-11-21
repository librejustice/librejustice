//! data.public.lu: the datasets of the Luxembourg Administration judiciaire, one yearly ZIP of
//! decisions in PDF or HTML each, read through the portal's API, and the text of an HTML decision.

use std::io::Read;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::strip_html;

pub const DATASETS_URL: &str =
    "https://data.public.lu/api/1/organizations/administration-judiciaire/datasets/?page_size=100";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(2);
/// A refusal waits this long before the request is asked again, twice at most.
const REFUSAL_PAUSE: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
struct Page {
    data: Vec<Dataset>,
    next_page: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dataset {
    pub slug: String,
    pub title: String,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resource {
    pub id: String,
    /// `2026.zip`.
    pub title: String,
    pub url: String,
    pub format: String,
    #[serde(default)]
    pub filesize: Option<u64>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Checksum {
    pub value: String,
}

/// A decision file of an archive, `pdf` or `html`.
pub struct Member {
    pub name: String,
    pub format: &'static str,
    pub bytes: Vec<u8>,
}

pub struct JusticeLuClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl JusticeLuClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "data.public.lu is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    /// One request at a time, two seconds apart; a 403, 429 or 503 pauses then asks again, and
    /// a third refusal stops the run.
    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let mut last = self.last.lock().await;
        for attempt in 0..3 {
            if let Some(at) = *last {
                tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
            }
            let response = self.http.get(url).send().await;
            *last = Some(Instant::now());
            let response = response?;
            let status = response.status().as_u16();
            if !matches!(status, 403 | 429 | 503) {
                return Ok(response.error_for_status()?.bytes().await?.to_vec());
            }
            if attempt == 2 {
                return Err(SourceError::Invalid(format!(
                    "data.public.lu answers {status} on {url}"
                )));
            }
            tracing::warn!(status, url, "data.public.lu refuses, the request waits");
            tokio::time::sleep(REFUSAL_PAUSE).await;
        }
        unreachable!("the third attempt returns")
    }

    /// Every dataset of the Administration judiciaire.
    pub async fn datasets(&self) -> Result<Vec<Dataset>> {
        let mut datasets = Vec::new();
        let mut next = Some(DATASETS_URL.to_string());
        while let Some(url) = next {
            let page: Page = serde_json::from_slice(&self.get(&url).await?)
                .map_err(|e| SourceError::Invalid(format!("data.public.lu datasets: {e}")))?;
            datasets.extend(page.data);
            next = page.next_page;
        }
        Ok(datasets)
    }

    pub async fn archive(&self, resource: &Resource) -> Result<Vec<u8>> {
        self.get(&resource.url).await
    }
}

/// The PDF and HTML files of an archive, in the order of its directory.
pub fn members(archive: &[u8]) -> Result<Vec<Member>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
    let mut out = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        let lower = name.to_lowercase();
        let format = if lower.ends_with(".pdf") {
            "pdf"
        } else if lower.ends_with(".html") {
            "html"
        } else {
            continue;
        };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        out.push(Member {
            name,
            format,
            bytes,
        });
    }
    Ok(out)
}

/// The decision an HTML file shows in its `reading-container` blocks, without the page's print
/// button, warning and loader; `None` when it holds no such block.
pub fn html_text(html: &str) -> Option<String> {
    const OPEN: &str = "class=\"reading-container\"";
    let mut blocks = Vec::new();
    let mut from = 0;
    while let Some(at) = html[from..].find(OPEN) {
        let start = html[..from + at].rfind("<div")?;
        let (inner, end) = div_inner(html, start)?;
        blocks.push(strip_html(inner));
        from = end;
    }
    (!blocks.is_empty()).then(|| blocks.join("\n"))
}

/// The inner HTML of the `<div` opening at `start`, nested divs included, and where it closes.
fn div_inner(html: &str, start: usize) -> Option<(&str, usize)> {
    let inner = start + html[start..].find('>')? + 1;
    let (mut depth, mut at) = (1, inner);
    while depth > 0 {
        let close = at + html[at..].find("</div>")?;
        match html[at..close].find("<div") {
            Some(open) => {
                depth += 1;
                at += open + 4;
            }
            None => {
                depth -= 1;
                at = close + 6;
            }
        }
    }
    Some((&html[inner..at - 6], at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_html_decision_is_its_reading_blocks_alone() {
        let html = r#"<body><div class="juano-doc"><div class="juano-doc-actions no-print"><button>Imprimer PDF</button></div>
<div class="doc-content"><div class="preview-header"><p class="preview-title">Texte pseudonymisé</p><p>Avertissement: Ce document pseudonymisé a une valeur purement informative.</p></div>
<div class="doc-fragment" data-fragment-key="0"><div data-v-12fde2e4="" class="reading-container"><div><div class="Section_1">
<p><span id="1">N° 174 / 2026 pénal</span></p><p><span id="2">du 15.10.2026</span></p>
<p><span id="15"><span anonymized="1">PERSONNE1.)</span>,</span><span id="16"> demeurant à L-<span anonymized="1">ADRESSE1.)</span>,</span></p>
</div></div></div></div>
<div class="doc-fragment" data-fragment-key="1"><div data-v-12fde2e4="" class="reading-container"><div><div class="Section_1"><p>Par ces motifs, rejette le pourvoi.</p></div></div></div></div>
</div><div class="loader">Chargement en cours...</div></div></body>"#;
        assert_eq!(
            html_text(html).unwrap(),
            "N° 174 / 2026 pénal\ndu 15.10.2026\nPERSONNE1.), demeurant à L-ADRESSE1.),\nPar ces motifs, rejette le pourvoi."
        );
        assert_eq!(
            html_text("<body><p>Chargement en cours...</p></body>"),
            None
        );
    }
}
