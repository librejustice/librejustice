//! Mistral OCR on the Vibe keys, paced on the one budget their account shares.

use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use lj_llm::mistral::{ocr_layout_with_retry, MistralClient, MistralError};
use lj_store::repository::DecisionRepository;
use serde_json::Value;
use tokio::sync::Mutex;

/// The Vibe keys share one budget per account.
const PAGES_PER_MINUTE: usize = 50;
/// Pages sent in one call, so that a call never outgrows the minute.
pub const BATCH: usize = 16;
const THROTTLE_WAITS: u32 = 3;
const MINUTE: Duration = Duration::from_secs(60);

/// The last minute's sends, shared by every step the process runs side by side.
static SENT: LazyLock<Mutex<VecDeque<(Instant, usize)>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

pub struct Ocr {
    client: MistralClient,
}

impl Ocr {
    /// `None` when the `mistral-vibe` pool is empty.
    pub async fn from_pool(repo: &DecisionRepository<'_>) -> Result<Option<Self>> {
        let keys = repo.pool_api_keys("mistral-vibe").await?;
        if keys.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            client: MistralClient::new(keys, "mistral-ocr-latest".to_string())?,
        }))
    }

    /// Waits until `pages` more fit in the last minute's budget.
    async fn admit(&self, pages: usize) {
        let mut sent = SENT.lock().await;
        loop {
            let now = Instant::now();
            while sent
                .front()
                .is_some_and(|&(at, _)| now.duration_since(at) >= MINUTE)
            {
                sent.pop_front();
            }
            let used: usize = sent.iter().map(|&(_, n)| n).sum();
            match sent.front() {
                Some(&(oldest, _)) if used + pages > PAGES_PER_MINUTE => {
                    tokio::time::sleep_until((oldest + MINUTE).into()).await
                }
                _ => break,
            }
        }
        sent.push_back((Instant::now(), pages));
    }

    /// One markdown per page of `pdf`, which holds `pages` pages, at most [`BATCH`].
    pub async fn pages(
        &self,
        pdf: &[u8],
        pages: usize,
        label: &str,
    ) -> Result<Vec<String>, MistralError> {
        Ok(self
            .layout(pdf, pages, label)
            .await?
            .iter()
            .map(markdown)
            .collect())
    }

    /// The pages of `pdf` as the OCR returns them, each with its blocks and their boxes.
    pub async fn layout(
        &self,
        pdf: &[u8],
        pages: usize,
        label: &str,
    ) -> Result<Vec<Value>, MistralError> {
        let mut waits = 0;
        loop {
            self.admit(pages).await;
            match ocr_layout_with_retry(&self.client, pdf, label).await {
                Err(MistralError::Status(429)) if waits < THROTTLE_WAITS => {
                    waits += 1;
                    tracing::warn!(label, waits, "ocr: throttled, waiting a minute");
                    tokio::time::sleep(MINUTE).await;
                }
                other => return other,
            }
        }
    }

    /// The whole of `pdf`, its pages joined by a line break.
    pub async fn document(&self, pdf: &[u8], label: &str) -> Result<String> {
        Ok(self
            .document_layout(pdf, label)
            .await?
            .iter()
            .map(markdown)
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Every page of `pdf` as [`Self::layout`] returns it, [`BATCH`] pages a call.
    pub async fn document_layout(&self, pdf: &[u8], label: &str) -> Result<Vec<Value>> {
        let bytes = std::sync::Arc::new(pdf.to_vec());
        let file = bytes.clone();
        let count = tokio::task::spawn_blocking(move || lj_sources::pdf::page_count(&file))
            .await
            .map_err(|e| anyhow!("page count task: {e}"))??;
        let mut read = Vec::with_capacity(count as usize);
        for first in (1..=count).step_by(BATCH) {
            let numbers: Vec<u32> = (first..=(first + BATCH as u32 - 1).min(count)).collect();
            let subset = if count as usize <= BATCH {
                bytes.to_vec()
            } else {
                let (file, numbers) = (bytes.clone(), numbers.clone());
                tokio::task::spawn_blocking(move || lj_sources::pdf::pages_extract(&file, &numbers))
                    .await
                    .map_err(|e| anyhow!("pages task: {e}"))??
            };
            let pages = self.layout(&subset, numbers.len(), label).await?;
            if pages.len() != numbers.len() {
                bail!(
                    "ocr {label}: {} pages read for {}",
                    pages.len(),
                    numbers.len()
                );
            }
            read.extend(pages);
        }
        Ok(read)
    }
}

fn markdown(page: &Value) -> String {
    page["markdown"].as_str().unwrap_or_default().to_string()
}
