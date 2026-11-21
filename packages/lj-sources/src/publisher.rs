//! A client for an official publisher read through the egress proxy: one request at a time, two
//! seconds apart unless the publisher asks for more, under the LibreJusticeBot user agent,
//! pausing when the publisher refuses.

use std::error::Error as _;
use std::fmt;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
const PACE: Duration = Duration::from_secs(2);
const PAUSES: [u64; 3] = [60, 300, 900];
const TIMEOUT: Duration = Duration::from_secs(300);

tokio::task_local! {
    /// The moment the running command must be done by. A pause is taken only if it and one more
    /// request end before it: past that, the publisher is given up rather than waited for.
    pub static GIVE_UP_BY: Instant;
}

fn fits(secs: u64) -> bool {
    GIVE_UP_BY
        .try_with(|by| Instant::now() + Duration::from_secs(secs) + TIMEOUT <= *by)
        .unwrap_or(true)
}

/// One consolidated state of a text: the day it applies from, its file, the day the publisher
/// last modified it when its listing says so, and the identifier of its content when the listing
/// names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consolidation {
    pub applies_from: String,
    pub xml_url: String,
    pub modified: Option<NaiveDate>,
    pub token: Option<u64>,
}

/// The day of an `xsd:dateTime` or `xsd:date` literal, as the publisher's clock reads it.
pub(crate) fn literal_day(literal: &str) -> Result<NaiveDate> {
    literal
        .get(..10)
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
        .ok_or_else(|| SourceError::Invalid(format!("not a date: {literal:?}")))
}

/// How a publisher refuses a request: with a status, or on the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Status(u16),
    /// An HTTP/2 reset or GOAWAY with ENHANCE_YOUR_CALM.
    Calm,
    /// The connection reset, or closed before any answer.
    Dropped,
}

impl Refusal {
    /// The refusal a failed send carries on its connection, if any.
    pub fn on_transport(e: &reqwest::Error) -> Option<Self> {
        std::iter::successors(e.source(), |&e| e.source()).find_map(|e| {
            if let Some(e) = e.downcast_ref::<h2::Error>() {
                (e.is_remote() && e.reason() == Some(h2::Reason::ENHANCE_YOUR_CALM))
                    .then_some(Self::Calm)
            } else if let Some(e) = e.downcast_ref::<hyper::Error>() {
                e.is_incomplete_message().then_some(Self::Dropped)
            } else {
                e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset)
                    .then_some(Self::Dropped)
            }
        })
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(f, "{status}"),
            Self::Calm => f.write_str("ENHANCE_YOUR_CALM"),
            Self::Dropped => f.write_str("a dropped connection"),
        }
    }
}

pub struct PacedClient {
    pub(crate) http: reqwest::Client,
    publisher: &'static str,
    pace: Duration,
    last: Mutex<Option<Instant>>,
}

impl PacedClient {
    pub fn new(proxy: &str, publisher: &'static str) -> Result<Self> {
        Self::build(proxy, publisher, false)
    }

    /// A client that keeps the cookies its publisher sets, for a search held in a session.
    pub fn with_cookies(proxy: &str, publisher: &'static str) -> Result<Self> {
        Self::build(proxy, publisher, true)
    }

    fn build(proxy: &str, publisher: &'static str, cookies: bool) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(format!(
                "{publisher} is read through LIBREJUSTICE_EGRESS_PROXY, which is empty"
            )));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .cookie_store(cookies)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(120))
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            publisher,
            pace: PACE,
            last: Mutex::new(None),
        })
    }

    /// Requests `pace` apart rather than two seconds.
    pub fn paced(self, pace: Duration) -> Self {
        Self { pace, ..self }
    }

    pub async fn text(&self, url: &str) -> Result<String> {
        let body = self.send(|| self.http.get(url)).await?;
        String::from_utf8(body)
            .map_err(|e| SourceError::Invalid(format!("{url} is not UTF-8: {e}")))
    }

    /// One request's status, paced, without pause nor retry.
    pub async fn status(&self, url: &str) -> Result<u16> {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep(self.pace.saturating_sub(at.elapsed())).await;
        }
        *last = Some(Instant::now());
        Ok(self.http.get(url).send().await?.status().as_u16())
    }

    /// A 403, a 429, a 5xx or a failed or stalled connection pauses then retries, three times or
    /// until a pause no longer fits before [`GIVE_UP_BY`]; then a 403, a 429 or a
    /// [`Refusal::on_transport`] is [`SourceError::Refused`], a 5xx [`SourceError::Invalid`],
    /// another connection failure its error.
    pub async fn send(&self, request: impl Fn() -> reqwest::RequestBuilder) -> Result<Vec<u8>> {
        let mut last = self.last.lock().await;
        let url = || {
            request()
                .build()
                .map(|r| r.url().to_string())
                .unwrap_or_default()
        };
        for (attempt, pause) in PAUSES.iter().map(Some).chain([None]).enumerate() {
            if let Some(at) = *last {
                tokio::time::sleep(self.pace.saturating_sub(at.elapsed())).await;
            }
            *last = Some(Instant::now());
            let response = match (request().send().await, pause) {
                (Ok(response), _) => response,
                (Err(e), Some(&secs)) if fits(secs) => {
                    tracing::warn!(
                        publisher = self.publisher,
                        attempt,
                        pause_s = secs,
                        error = %e,
                        "publisher unreachable, pausing"
                    );
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    continue;
                }
                (Err(e), _) => {
                    return Err(match Refusal::on_transport(&e) {
                        Some(refusal) => SourceError::Refused {
                            publisher: self.publisher,
                            refusal,
                            url: url(),
                        },
                        None => e.into(),
                    })
                }
            };
            let status = response.status().as_u16();
            match (status, pause) {
                (200 | 206, _) => return Ok(response.bytes().await?.to_vec()),
                (403 | 429 | 500..=599, Some(&secs)) if fits(secs) => {
                    tracing::warn!(
                        publisher = self.publisher,
                        status,
                        attempt,
                        pause_s = secs,
                        "publisher refuses, pausing"
                    );
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
                (403 | 429, _) => {
                    return Err(SourceError::Refused {
                        publisher: self.publisher,
                        refusal: Refusal::Status(status),
                        url: url(),
                    })
                }
                _ => {
                    return Err(SourceError::Invalid(format!(
                        "{} answers {status} for {} after {attempt} pauses",
                        self.publisher,
                        url()
                    )))
                }
            }
        }
        unreachable!("the last attempt returns")
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use std::time::{Duration, Instant};

    use super::{PacedClient, Refusal, GIVE_UP_BY};

    async fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        (listener, url)
    }

    #[tokio::test]
    async fn a_reset_asking_for_calm_or_a_dropped_connection_is_a_refusal() {
        let (h2c, h1) = (
            reqwest::Client::builder()
                .no_proxy()
                .http2_prior_knowledge()
                .build()
                .unwrap(),
            reqwest::Client::builder().no_proxy().build().unwrap(),
        );

        let (calm, url) = listener().await;
        tokio::spawn(async move {
            let mut socket = calm.accept().await.unwrap().0;
            socket.read_exact(&mut [0; 24]).await.unwrap();
            loop {
                let mut head = [0; 9];
                socket.read_exact(&mut head).await.unwrap();
                let len = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
                socket.read_exact(&mut vec![0; len]).await.unwrap();
                if head[3] == 0x1 {
                    break;
                }
            }
            // An empty SETTINGS, then RST_STREAM on stream 1 with ENHANCE_YOUR_CALM.
            let frames = [
                0, 0, 0, 0x4, 0, 0, 0, 0, 0, 0, 0, 4, 0x3, 0, 0, 0, 0, 1, 0, 0, 0, 11,
            ];
            socket.write_all(&frames).await.unwrap();
            let _ = socket.read(&mut [0; 4096]).await;
        });
        let e = h2c.get(url).send().await.unwrap_err();
        assert_eq!(Refusal::on_transport(&e), Some(Refusal::Calm));

        let (empty, url) = listener().await;
        tokio::spawn(async move {
            let mut socket = empty.accept().await.unwrap().0;
            let _ = socket.read(&mut [0; 4096]).await;
        });
        let e = h1.get(url).send().await.unwrap_err();
        assert_eq!(Refusal::on_transport(&e), Some(Refusal::Dropped));

        let (closed, url) = listener().await;
        drop(closed);
        let e = h1.get(url).send().await.unwrap_err();
        assert_eq!(Refusal::on_transport(&e), None);
    }

    #[tokio::test]
    async fn a_publisher_that_stops_answering_is_given_up_before_the_command_deadline() {
        let (closed, proxy) = listener().await;
        drop(closed);
        let client = PacedClient::new(&proxy, "closed").unwrap();
        let by = Instant::now() + Duration::from_secs(300);
        let read = GIVE_UP_BY.scope(by, client.text("http://publisher.invalid/"));
        let read = tokio::time::timeout(Duration::from_secs(10), read).await;
        assert!(matches!(read, Ok(Err(_))));
    }
}
