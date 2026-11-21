//! Rechtspraak open data (`data.rechtspraak.nl`): the ECLI index searched by decision or
//! modification days, and each document read one at a time.

use std::time::{Duration, Instant};

use lj_parse::RechtspraakListing;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://data.rechtspraak.nl";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// The service asks for no concurrent requests and gives no rate: an answer takes about 0.1 s.
pub const PACE: Duration = Duration::from_secs(1);
/// The pace once the service has refused twice.
pub const SLOW_PACE: Duration = Duration::from_secs(2);
/// The most entries a search answers.
pub const PAGE: usize = 1_000;

/// The pauses before a refused request (429, 503) is sent again; past the last, the run stops.
const REFUSAL_PAUSES: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(300),
    Duration::from_secs(900),
];
/// The pauses before a request whose connection failed is sent again.
const RETRY_PAUSES: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

struct Pacer {
    last: Option<(Instant, Duration)>,
    refusals: usize,
}

pub struct RechtspraakClient {
    http: reqwest::Client,
    pacer: Mutex<Pacer>,
}

impl RechtspraakClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "data.rechtspraak.nl is read through LIBREJUSTICE_EGRESS_PROXY, which is empty"
                    .into(),
            ));
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .proxy(reqwest::Proxy::all(proxy)?)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()?,
            pacer: Mutex::new(Pacer {
                last: None,
                refusals: 0,
            }),
        })
    }

    /// One request at a time, [`PACE`] apart, or twice the last answer's time when longer.
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<(u16, String)> {
        let mut pacer = self.pacer.lock().await;
        let (mut refusals, mut retries) = (REFUSAL_PAUSES.iter(), RETRY_PAUSES.iter());
        loop {
            let pace = if pacer.refusals >= 2 { SLOW_PACE } else { PACE };
            if let Some((at, took)) = pacer.last {
                tokio::time::sleep(pace.max(took * 2).saturating_sub(at.elapsed())).await;
            }
            let started = Instant::now();
            let answer = self
                .http
                .get(format!("{BASE_URL}{path}"))
                .query(query)
                .send()
                .await;
            let answer = match answer {
                Ok(response) => {
                    let status = response.status().as_u16();
                    response.text().await.map(|body| (status, body))
                }
                Err(e) => Err(e),
            };
            pacer.last = Some((Instant::now(), started.elapsed()));
            match answer {
                Ok((status @ (429 | 503), _)) => {
                    pacer.refusals += 1;
                    let Some(pause) = refusals.next() else {
                        return Err(SourceError::Invalid(format!(
                            "data.rechtspraak.nl answers {status} on {path} after {} pauses",
                            REFUSAL_PAUSES.len()
                        )));
                    };
                    tracing::warn!(
                        status,
                        path,
                        pause_s = pause.as_secs(),
                        refusals = pacer.refusals,
                        "rechtspraak: refused, sent again after a pause"
                    );
                    tokio::time::sleep(*pause).await;
                }
                Ok(answer) => return Ok(answer),
                Err(e) => match retries.next() {
                    Some(pause) if !e.is_builder() => {
                        tracing::warn!(path, error = %e, pause_s = pause.as_secs(), "rechtspraak: transport error, sent again after a pause");
                        tokio::time::sleep(*pause).await;
                    }
                    _ => return Err(e.into()),
                },
            }
        }
    }

    /// A page of the ECLI index: `query` holds the criteria, `max` and `from` are added.
    pub async fn search(
        &self,
        query: &[(&str, String)],
        from: usize,
    ) -> Result<RechtspraakListing> {
        let mut query = query.to_vec();
        query.push(("max", PAGE.to_string()));
        query.push(("from", from.to_string()));
        let (status, body) = self.get("/uitspraken/zoeken", &query).await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "data.rechtspraak.nl answers {status} on a search"
            )));
        }
        lj_parse::rechtspraak_listing(&body).map_err(|e| SourceError::Invalid(e.to_string()))
    }

    /// The document of an ECLI, `None` when the service knows none.
    pub async fn document(&self, ecli: &str) -> Result<Option<String>> {
        let (status, body) = self
            .get("/uitspraken/content", &[("id", ecli.to_string())])
            .await?;
        match status {
            200 => Ok(Some(body)),
            404 => Ok(None),
            _ => Err(SourceError::Invalid(format!(
                "data.rechtspraak.nl answers {status} on {ecli}"
            ))),
        }
    }
}
