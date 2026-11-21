//! A2AJ's Canadian decisions on Hugging Face: one parquet file per court, read row by row from disk.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::reader::RowIter;
use parquet::schema::types::Type;
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;

use crate::error::{Result, SourceError};
use crate::opencaselaw::USER_AGENT;
use crate::parquet_rows::{invalid, json_row};

const DATA_URL: &str = "https://huggingface.co/datasets/a2aj/canadian-case-law/resolve/main";
const TREE_URL: &str = "https://huggingface.co/api/datasets/a2aj/canadian-case-law/tree/main";

/// The columns read: the citation lists, the largest after the texts, are left.
const COLUMNS: [&str; 11] = [
    "dataset",
    "citation_en",
    "citation_fr",
    "name_en",
    "name_fr",
    "document_date_en",
    "document_date_fr",
    "url_en",
    "url_fr",
    "unofficial_text_en",
    "unofficial_text_fr",
];

pub struct A2ajClient {
    http: reqwest::Client,
}

impl A2ajClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "the A2AJ dataset is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { http })
    }

    /// The sha256 of each court's file by court, `FC`, from the repository's tree in one request.
    pub async fn checksums(&self) -> Result<HashMap<String, String>> {
        let url = format!("{TREE_URL}?recursive=true");
        let response = self.http.get(&url).send().await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SourceError::Invalid(format!("{url} answers {status}")));
        }
        let tree: Vec<Value> = response.json().await?;
        Ok(tree
            .iter()
            .filter_map(|e| {
                let court = e.get("path")?.as_str()?.strip_suffix("/train.parquet")?;
                let oid = e.pointer("/lfs/oid")?.as_str()?;
                Some((court.to_string(), oid.to_string()))
            })
            .collect())
    }

    /// Writes a court's file to `path`, through a file beside it renamed once whole.
    pub async fn download(&self, court: &str, path: &Path) -> Result<u64> {
        let url = format!("{DATA_URL}/{court}/train.parquet");
        let mut response = self.http.get(&url).send().await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SourceError::Invalid(format!("{url} answers {status}")));
        }
        let part = path.with_extension("part");
        let mut file = tokio::fs::File::create(&part).await?;
        let mut size = 0u64;
        while let Some(chunk) = response.chunk().await? {
            size += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        tokio::fs::rename(&part, path).await?;
        Ok(size)
    }
}

/// The rows of a court's file, each its non-null columns of [`COLUMNS`] by name.
pub fn rows(path: &Path) -> Result<impl Iterator<Item = Result<Map<String, Value>>>> {
    let reader = SerializedFileReader::new(std::fs::File::open(path)?).map_err(invalid)?;
    let schema = reader.metadata().file_metadata().schema();
    let fields = schema
        .get_fields()
        .iter()
        .filter(|f| COLUMNS.contains(&f.name()))
        .cloned()
        .collect();
    let projection = Type::group_type_builder(schema.name())
        .with_fields(fields)
        .build()
        .map_err(invalid)?;
    let rows = RowIter::from_file_into(Box::new(reader))
        .project(Some(projection))
        .map_err(invalid)?;
    Ok(rows.map(|row| json_row(&row.map_err(invalid)?)))
}
