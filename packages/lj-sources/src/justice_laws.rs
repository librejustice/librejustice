//! Justice Laws, Canada's consolidated federal Acts and regulations: the index of their
//! languages over HTTPS, and every point-in-time state of each in each language from the
//! Department of Justice's FTP server through a CONNECT tunnel on the egress proxy.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::publisher::{Consolidation, PacedClient};

/// The host the open.canada.ca record of the point-in-time bulk XML names.
pub const FTP_HOST: &str = "205.193.86.89";
/// Every Act and regulation in force, in both languages.
pub const INDEX_URL: &str = "https://laws-lois.justice.gc.ca/eng/XML/Legis.xml";
const ROOT: &str = "/PITXML";
const PACE: Duration = Duration::from_secs(2);
const PAUSES: [u64; 3] = [60, 300, 900];
const DEADLINE: Duration = Duration::from_secs(600);

/// The four collections, one per kind and language.
pub const COLLECTIONS: [&str; 4] = ["lois", "acts", "reglements", "regulations"];

static STATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{4})(\d{2})(\d{2})$").unwrap());
static PASSIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+),(\d+),(\d+),(\d+),(\d+),(\d+)").unwrap());
/// A line of the server's listing: `09-11-26  09:20AM       <DIR>          20260718`.
static LISTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{2})-(\d{2})-(\d{2})\s+(\d{2}):(\d{2})(AM|PM)\s+(?:<DIR>|\d+)\s+(.+?)\s*$")
        .unwrap()
});

/// A folder of a listing and the minute it last changed, as the server's clock reads it: a
/// collection's folder changes when a state is added to it, a state's when its file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub changed: NaiveDate,
    /// `yyyymmddhhmm` of that change.
    pub stamp: u64,
}

fn listed(listing: &[u8]) -> Vec<Listed> {
    String::from_utf8_lossy(listing)
        .lines()
        .filter_map(|line| {
            let c = LISTED.captures(line.trim_end())?;
            let n = |i: usize| c[i].parse::<u32>().ok();
            let (month, day, year) = (n(1)?, n(2)?, 2000 + n(3)? as i32);
            let hour = match (n(4)? % 12, &c[6]) {
                (h, "PM") => h + 12,
                (h, _) => h,
            };
            let changed = NaiveDate::from_ymd_opt(year, month, day)?;
            Some(Listed {
                name: c[7].to_string(),
                changed,
                stamp: ((year as u64 * 100 + u64::from(month)) * 100 + u64::from(day)) * 10_000
                    + u64::from(hour) * 100
                    + u64::from(n(5)?),
            })
        })
        .collect()
}

fn invalid(what: String) -> SourceError {
    SourceError::Invalid(format!("justice laws ftp: {what}"))
}

enum Fail {
    Reply(u16, String),
    Other(SourceError),
}

impl<E: Into<SourceError>> From<E> for Fail {
    fn from(e: E) -> Self {
        Self::Other(e.into())
    }
}

impl Fail {
    /// Too many connections, a data connection refused or cut, a file busy.
    fn transient(&self) -> bool {
        matches!(self, Self::Reply(421 | 425 | 426 | 450, _))
    }

    fn into_error(self) -> SourceError {
        match self {
            Self::Reply(code, text) => invalid(format!("{code} {}", text.trim())),
            Self::Other(e) => e,
        }
    }
}

struct Session {
    control: BufReader<TcpStream>,
}

impl Session {
    async fn reply(&mut self) -> std::result::Result<(u16, String), Fail> {
        let mut text = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            if self.control.read_line(&mut line).await? == 0 {
                return Err(Fail::Other(invalid("control connection closed".into())));
            }
            text.push_str(&line);
            let b = line.as_bytes();
            if b.len() >= 4 && b[..3].iter().all(u8::is_ascii_digit) && b[3] == b' ' {
                return Ok((line[..3].parse().unwrap_or(0), text));
            }
        }
    }

    async fn command(
        &mut self,
        command: &str,
        expect: &[u16],
    ) -> std::result::Result<String, Fail> {
        self.control
            .get_mut()
            .write_all(format!("{command}\r\n").as_bytes())
            .await?;
        let (code, text) = self.reply().await?;
        if expect.contains(&code) {
            Ok(text)
        } else {
            Err(Fail::Reply(code, text))
        }
    }
}

pub struct JusticeLawsClient {
    proxy: String,
    last: Mutex<Option<Instant>>,
    http: PacedClient,
}

impl JusticeLawsClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(invalid(
                "the FTP server is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let url = url::Url::parse(proxy)
            .map_err(|e| invalid(format!("LIBREJUSTICE_EGRESS_PROXY: {e}")))?;
        let host = url
            .host_str()
            .ok_or_else(|| invalid("LIBREJUSTICE_EGRESS_PROXY has no host".into()))?;
        let port = url.port_or_known_default().unwrap_or(80);
        Ok(Self {
            proxy: format!("{host}:{port}"),
            last: Mutex::new(None),
            http: PacedClient::new(proxy, "justice_laws")?,
        })
    }

    /// `Legis.xml`, the index of every Act and regulation in force in both languages.
    pub async fn index(&self) -> Result<String> {
        let text = self.http.text(INDEX_URL).await?;
        Ok(text.trim_start_matches('\u{feff}').to_string())
    }

    async fn tunnel(&self, host: &str, port: u16) -> std::result::Result<TcpStream, Fail> {
        let mut stream = TcpStream::connect(&self.proxy).await?;
        let connect = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
        stream.write_all(connect.as_bytes()).await?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).await? == 0 {
                return Err(Fail::Other(invalid("the proxy closed the tunnel".into())));
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head);
        let status = head.lines().next().unwrap_or_default();
        if status.split_whitespace().nth(1) != Some("200") {
            return Err(Fail::Other(invalid(format!(
                "the proxy answers {status:?} to CONNECT {host}:{port}"
            ))));
        }
        Ok(stream)
    }

    /// `command` (`LIST` or `RETR`) in its own session, on a passive data connection.
    async fn transfer(&self, command: &str) -> std::result::Result<Vec<u8>, Fail> {
        let mut session = Session {
            control: BufReader::new(self.tunnel(FTP_HOST, 21).await?),
        };
        let (code, text) = session.reply().await?;
        if code != 220 {
            return Err(Fail::Reply(code, text));
        }
        session.command("USER anonymous", &[230, 331]).await?;
        session.command("PASS anonymous@", &[230, 202]).await?;
        session.command("TYPE I", &[200]).await?;
        let text = session.command("PASV", &[227]).await?;
        let p = PASSIVE
            .captures(&text)
            .ok_or_else(|| Fail::Other(invalid(format!("no address in {text:?}"))))?;
        let octet = |i: usize| p[i].parse::<u16>().unwrap_or(0);
        let host = format!("{}.{}.{}.{}", &p[1], &p[2], &p[3], &p[4]);
        let mut data = self.tunnel(&host, octet(5) * 256 + octet(6)).await?;
        session.command(command, &[125, 150]).await?;
        let mut body = Vec::new();
        data.read_to_end(&mut body).await?;
        let (code, text) = session.reply().await?;
        if code != 226 && code != 250 {
            return Err(Fail::Reply(code, text));
        }
        session.command("QUIT", &[221]).await.ok();
        Ok(body)
    }

    /// One transfer at a time, `PACE` apart; a transient refusal pauses then retries, three times.
    async fn paced(&self, command: &str) -> Result<Vec<u8>> {
        let mut last = self.last.lock().await;
        for (attempt, pause) in PAUSES.iter().map(Some).chain([None]).enumerate() {
            if let Some(at) = *last {
                tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
            }
            *last = Some(Instant::now());
            let outcome = tokio::time::timeout(DEADLINE, self.transfer(command))
                .await
                .map_err(|_| invalid(format!("{command} took over {DEADLINE:?}")))?;
            match (outcome, pause) {
                (Ok(body), _) => return Ok(body),
                (Err(fail), Some(&secs)) if fail.transient() => {
                    let why = fail.into_error().to_string();
                    tracing::warn!(
                        attempt,
                        pause_s = secs,
                        why,
                        "justice laws ftp refuses, pausing"
                    );
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
                (Err(fail), _) => return Err(fail.into_error()),
            }
        }
        unreachable!("the last attempt returns")
    }

    /// The folders of `collection`, one of [`COLLECTIONS`], each an instrument.
    pub async fn folders(&self, collection: &str) -> Result<Vec<Listed>> {
        if !COLLECTIONS.contains(&collection) {
            return Err(invalid(format!("not a collection: {collection:?}")));
        }
        Ok(listed(
            &self.paced(&format!("LIST {ROOT}/{collection}")).await?,
        ))
    }

    /// The point-in-time states of the instrument in `folder` of `collection`, oldest first, each
    /// keyed by the minute its folder last changed.
    pub async fn consolidations(
        &self,
        collection: &str,
        folder: &str,
    ) -> Result<Vec<Consolidation>> {
        if !COLLECTIONS.contains(&collection)
            || folder.is_empty()
            || folder.contains(['/', '\r', '\n'])
        {
            return Err(invalid(format!(
                "not an instrument folder: {collection}/{folder}"
            )));
        }
        let listing = self
            .paced(&format!("LIST {ROOT}/{collection}/{folder}"))
            .await?;
        let mut states: Vec<Consolidation> = listed(&listing)
            .into_iter()
            .filter_map(|l| {
                let d = STATE.captures(&l.name)?;
                Some(Consolidation {
                    applies_from: format!("{}-{}-{}", &d[1], &d[2], &d[3]),
                    xml_url: format!("{ROOT}/{collection}/{folder}/{}/{folder}.xml", l.name),
                    modified: Some(l.changed),
                    token: Some(l.stamp),
                })
            })
            .collect();
        states.sort_by(|a, b| a.applies_from.cmp(&b.applies_from));
        Ok(states)
    }

    /// The XML at `path`, as [`Self::consolidations`] names it.
    pub async fn xml(&self, path: &str) -> Result<String> {
        let body = self.paced(&format!("RETR {path}")).await?;
        let text =
            String::from_utf8(body).map_err(|e| invalid(format!("{path} is not UTF-8: {e}")))?;
        Ok(text.trim_start_matches('\u{feff}').to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_line_gives_its_folder_and_the_minute_it_changed() {
        let listing = b"09-11-26  09:20AM       <DIR>          20260718\r\n10-15-19  02:30PM       <DIR>          C.R.C.,_c._221\r\n05-26-21  11:31AM              7032253 A-1_PITSections.xml\r\n";
        let got: Vec<(String, String, u64)> = listed(listing)
            .into_iter()
            .map(|l| (l.name, l.changed.to_string(), l.stamp))
            .collect();
        assert_eq!(
            got,
            [
                ("20260718".into(), "2026-09-11".into(), 202609110920),
                ("C.R.C.,_c._221".into(), "2019-10-15".into(), 201910151430),
                (
                    "A-1_PITSections.xml".into(),
                    "2021-05-26".into(),
                    202105261131
                ),
            ]
        );
    }
}
