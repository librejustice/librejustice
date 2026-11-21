//! The parliamentary questions' sources, through the egress proxy: Assemblée, DILA, Sénat.

use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const AN_REPOSITORY: &str = "https://data.assemblee-nationale.fr/static/openData/repository";
pub const DILA_AN: &str = "https://echanges.dila.gouv.fr/OPENDATA/Questions-Reponses/AN";
pub const SENAT_DUMP: &str = "https://data.senat.fr/data/questions/questions.zip";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
const PACE: Duration = Duration::from_secs(2);
const ATTEMPTS: u32 = 5;
/// Resumptions in a row that bring nothing before a transfer fails, each wait twice the last.
const STALLS: u32 = 8;

/// The Assemblée's three question sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnSet {
    Written,
    Government,
    OralWithoutDebate,
}

impl AnSet {
    pub const ALL: [AnSet; 3] = [AnSet::Written, AnSet::Government, AnSet::OralWithoutDebate];

    /// The set's archive in a legislature: the 14th and the 15th carry their numeral, later ones none.
    pub fn archive(self, legislature: u16) -> String {
        let (dir, file) = match self {
            AnSet::Written => ("questions_ecrites", "Questions_ecrites"),
            AnSet::Government => ("questions_gouvernement", "Questions_gouvernement"),
            AnSet::OralWithoutDebate => {
                ("questions_orales_sans_debat", "Questions_orales_sans_debat")
            }
        };
        let numeral = match legislature {
            14 => "_XIV",
            15 => "_XV",
            _ => "",
        };
        format!("{legislature}/questions/{dir}/{file}{numeral}.json.zip")
    }
}

pub struct QuestionsClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl QuestionsClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "the questions are read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            // The Assemblée's server cut HTTP/2 transfers mid-file through the gateway on 18/11/2026.
            .http1_only()
            // DILA serves its `.taz` as `Content-Encoding: gzip`: an issue is kept as the file it is.
            .no_gzip()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(1800))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    /// Sends at the pace, again after a network failure or a 5xx, each wait one pace longer.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        for attempt in 1.. {
            if let Some(at) = *last {
                tokio::time::sleep((PACE * attempt).saturating_sub(at.elapsed())).await;
            }
            let sent = request
                .try_clone()
                .expect("a request without a streamed body")
                .send()
                .await;
            *last = Some(Instant::now());
            match sent.and_then(reqwest::Response::error_for_status) {
                Err(e)
                    if attempt < ATTEMPTS
                        && (e.is_connect()
                            || e.is_timeout()
                            || e.is_request()
                            || e.status().is_some_and(|s| s.is_server_error())) =>
                {
                    tracing::warn!(attempt, error = %e, "questions: request failed, sent again");
                }
                sent => return Ok(sent?),
            }
        }
        unreachable!("the attempts loop returns")
    }

    pub async fn text(&self, url: &str) -> Result<String> {
        Ok(self.send(self.http.get(url)).await?.text().await?)
    }

    pub async fn bytes(&self, url: &str) -> Result<Vec<u8>> {
        Ok(self.send(self.http.get(url)).await?.bytes().await?.to_vec())
    }

    /// `url` into `dest` unless the copy kept is current by `Last-Modified`, a cut transfer resumed by `Range`; whether it came.
    pub async fn file(&self, url: &str, dest: &Path) -> Result<bool> {
        let stamp_path = dest.with_extension("last-modified");
        let mut request = self.http.get(url);
        if dest.exists() {
            if let Ok(stamp) = tokio::fs::read_to_string(&stamp_path).await {
                request = request.header(reqwest::header::IF_MODIFIED_SINCE, stamp.trim());
            }
        }
        let mut response = self.send(request).await?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(false);
        }
        let stamp = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let total = response.content_length();
        let part = dest.with_extension("part");
        let mut out = tokio::fs::File::create(&part).await?;
        let (mut have, mut stalls) = (0u64, 0);
        loop {
            let before = have;
            let cut = loop {
                match response.chunk().await {
                    Ok(Some(bytes)) => {
                        out.write_all(&bytes).await?;
                        have += bytes.len() as u64;
                    }
                    Ok(None) => break None,
                    Err(e) => break Some(e),
                }
            };
            let Some(error) = cut else { break };
            stalls = if have > before { 0 } else { stalls + 1 };
            if stalls == STALLS {
                return Err(error.into());
            }
            tracing::debug!(url, have, error = %error, "questions: transfer cut, resumed");
            // The Assemblée's server served empty ranges for a minute at a time on 18/11/2026.
            tokio::time::sleep(PACE * (1 << stalls)).await;
            let range = self
                .http
                .get(url)
                .header(reqwest::header::RANGE, format!("bytes={have}-"));
            response = self.send(range).await?;
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(SourceError::Invalid(format!(
                    "{url}: no range served after a cut"
                )));
            }
        }
        out.flush().await?;
        if total.is_some_and(|t| t != have) {
            return Err(SourceError::Invalid(format!(
                "{url}: {have} bytes of {total:?}"
            )));
        }
        tokio::fs::rename(&part, dest).await?;
        match stamp {
            Some(stamp) => tokio::fs::write(&stamp_path, stamp).await?,
            None => tokio::fs::remove_file(&stamp_path).await.or_else(|e| {
                (e.kind() == std::io::ErrorKind::NotFound)
                    .then_some(())
                    .ok_or(e)
            })?,
        }
        Ok(true)
    }
}

/// Each record of an Assemblée archive, the object under `question`: one file per question under
/// `json/`, or the 14th's single file of the whole legislature, streamed.
pub fn an_records(zip: &Path, mut each: impl FnMut(Value)) -> Result<usize> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip)?)?;
    let mut count = 0;
    for i in 0..archive.len() {
        let member = archive.by_index(i)?;
        if !member.name().ends_with(".json") {
            continue;
        }
        let name = member.name().to_string();
        if !name.starts_with("json/") {
            count += Outer(&mut each)
                .deserialize(&mut serde_json::Deserializer::from_reader(BufReader::new(
                    member,
                )))
                .map_err(|e| SourceError::Invalid(format!("{name}: {e}")))?;
            continue;
        }
        let mut record: Value = serde_json::from_reader(member)
            .map_err(|e| SourceError::Invalid(format!("{name}: {e}")))?;
        match record.get_mut("question").map(Value::take) {
            Some(question) => {
                each(question);
                count += 1;
            }
            None => return Err(SourceError::Invalid(format!("{name}: no question"))),
        }
    }
    Ok(count)
}

/// `{"questionsEcrites": {"question": [...]}}`, each element handed over as it is read.
struct Outer<'f, F>(&'f mut F);
struct Middle<'f, F>(&'f mut F);
struct Items<'f, F>(&'f mut F);

impl<'de, F: FnMut(Value)> DeserializeSeed<'de> for Outer<'_, F> {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<usize, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de, F: FnMut(Value)> Visitor<'de> for Outer<'_, F> {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("the archive's root object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<usize, A::Error> {
        let mut count = 0;
        while map.next_key::<IgnoredAny>()?.is_some() {
            count += map.next_value_seed(Middle(&mut *self.0))?;
        }
        Ok(count)
    }
}

impl<'de, F: FnMut(Value)> DeserializeSeed<'de> for Middle<'_, F> {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<usize, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de, F: FnMut(Value)> Visitor<'de> for Middle<'_, F> {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an object holding `question`")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<usize, A::Error> {
        let mut count = 0;
        while let Some(key) = map.next_key::<String>()? {
            if key == "question" {
                count += map.next_value_seed(Items(&mut *self.0))?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(count)
    }
}

impl<'de, F: FnMut(Value)> DeserializeSeed<'de> for Items<'_, F> {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<usize, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, F: FnMut(Value)> Visitor<'de> for Items<'_, F> {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("the array of questions")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<usize, A::Error> {
        let mut count = 0;
        while let Some(question) = seq.next_element::<Value>()? {
            (self.0)(question);
            count += 1;
        }
        Ok(count)
    }
}

/// The `.taz` issues a folder of DILA's lists, by name.
pub fn dila_issue_names(listing: &str) -> Vec<String> {
    let mut names: Vec<String> = regex::Regex::new(r#"href="(ANQ\d+\.taz)""#)
        .expect("valid regex")
        .captures_iter(listing)
        .map(|c| c[1].to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The `XML1JO_AN*.xml` member of an issue's `.taz`, a tar under gzip, or `compress(1)` until early 2012.
pub fn dila_issue_xml(taz: &[u8]) -> Result<Vec<u8>> {
    let tar = crate::gazette::unlzw(taz)?;
    let mut archive = tar::Archive::new(tar.as_slice());
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if path
            .rsplit('/')
            .next()
            .is_some_and(|n| n.starts_with("XML1JO_"))
        {
            let mut xml = Vec::new();
            entry.read_to_end(&mut xml)?;
            return Ok(xml);
        }
    }
    Err(SourceError::Invalid("issue without XML1JO member".into()))
}

/// Each line of the Sénat dump, read as a stream.
pub fn senat_lines(zip: &Path, mut each: impl FnMut(&str)) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip)?)?;
    let member = archive.by_name("questions.sql")?;
    let mut reader = BufReader::with_capacity(1 << 20, member);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        each(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fourteenth_single_file_hands_over_each_question_in_turn() {
        let json = r#"{"questionsGvt": {"@xmlns": "x", "question": [{"uid": "A"}, {"uid": "B"}]}}"#;
        let mut seen = Vec::new();
        let count = Outer(&mut |q: Value| seen.push(q["uid"].as_str().unwrap().to_string()))
            .deserialize(&mut serde_json::Deserializer::from_str(json))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(seen, ["A", "B"]);
        assert_eq!(
            AnSet::OralWithoutDebate.archive(15),
            "15/questions/questions_orales_sans_debat/Questions_orales_sans_debat_XV.json.zip"
        );
    }
}
