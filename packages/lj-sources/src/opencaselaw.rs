//! OpenCaseLaw's dataset of Swiss decisions on Hugging Face, released under CC0: one parquet
//! file per court, read whole.

use std::time::Duration;

use parquet::file::reader::{FileReader, SerializedFileReader};
use serde_json::{Map, Value};

use crate::error::{Result, SourceError};
use crate::parquet_rows::{invalid, json_row};

pub const DATA_URL: &str = "https://huggingface.co/datasets/voilaj/swiss-caselaw/resolve/main/data";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";

pub struct OpenCaseLawClient {
    http: reqwest::Client,
}

impl OpenCaseLawClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "the OpenCaseLaw dataset is read through LIBREJUSTICE_EGRESS_PROXY, which is empty"
                    .into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self { http })
    }

    /// The rows of a court's file, `ow_gerichte`, each its non-null fields by column name.
    pub async fn court_rows(&self, court: &str) -> Result<Vec<Map<String, Value>>> {
        let url = format!("{DATA_URL}/{court}.parquet");
        let response = self.http.get(&url).send().await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SourceError::Invalid(format!("{url} answers {status}")));
        }
        rows(response.bytes().await?)
    }
}

fn rows(
    bytes: impl parquet::file::reader::ChunkReader + 'static,
) -> Result<Vec<Map<String, Value>>> {
    let reader = SerializedFileReader::new(bytes).map_err(invalid)?;
    reader
        .get_row_iter(None)
        .map_err(invalid)?
        .map(|row| json_row(&row.map_err(invalid)?))
        .collect()
}
