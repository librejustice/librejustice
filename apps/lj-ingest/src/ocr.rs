//! Mistral OCR on the Vibe keys, paced on the budget each account's keys share. Mistral's answer
//! is kept in the bucket, and the text is read from it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::Hash;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use lj_bucket::Bucket;
use lj_llm::mistral::{ocr_layout_with_retry, MistralClient, MistralError};
use lj_store::repository::DecisionRepository;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// The Vibe keys share one budget per account.
const PAGES_PER_MINUTE: usize = 50;
/// Pages sent in one call, so that a call never outgrows the minute.
pub const BATCH: usize = 16;
const THROTTLE_WAITS: usize = 3;
const MINUTE: Duration = Duration::from_secs(60);

/// What the OCR writes on a stamp or a blank, and no French page holds.
static INVENTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\x{3040}-\x{9FFF}\x{AC00}-\x{D7AF}]|\\therefore|\[Non-Text\]|Ground Truth image|The following table provides|The image (?:contains|is too)|(?:\b\d{1,2}\.\s+){20}|\{\x22box",
    )
    .unwrap()
});
static FRACTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\frac\s*\{[^{}]*\}\s*\{[^{}]*\}").unwrap());
/// Measured on a quarter of the RAA pages read: a table repeats a line 22 times on a page at
/// most, a loop 43 times and more.
const LOOP_LINES: usize = 30;
/// A formula repeats a fraction twice at most, a loop `\frac{1}{2}` ten times and more.
const LOOP_FRACTIONS: usize = 5;
/// Under 0.1% of pixels darker than grey 160 at 75 dpi: 5,230 of 40,268 scanned circulaire pages,
/// 9 of them a lone line, none body text. At 25 dpi thin print greys out and pages of it pass.
const BLANK_INK: f64 = 0.001;
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}{3,}").unwrap());
static IMAGE_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());

/// A page's text as the OCR writes it on a blank: 20 characters or fewer, fewer than three words,
/// or a formula. It holds 657 of the 671 answers on circulaire pages under [`BLANK_INK`].
pub fn suspect(text: &str) -> bool {
    let text = IMAGE_LINK.replace_all(text, "");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    !text.is_empty()
        && (text.chars().count() <= 20
            || WORD.find_iter(&text).count() < 3
            || text.contains("\\(")
            || text.contains("\\["))
}

/// `page` as the OCR returns a page with no text, its index and dimensions kept.
pub fn emptied(page: &Value) -> Value {
    serde_json::json!({
        "index": page["index"],
        "markdown": "",
        "images": [],
        "tables": [],
        "blocks": [],
        "dimensions": page["dimensions"],
    })
}

/// What a pass emptying the blank pages of kept answers found.
#[derive(Default)]
pub struct Emptied {
    pub docs_read: usize,
    pub docs: usize,
    pub redrawn: usize,
    pub pages: usize,
    pub chars: usize,
    pub pdf_bytes: usize,
    pub gone: usize,
}

impl Emptied {
    /// Counts a page emptied of `text`.
    pub fn empty(&mut self, text: &str) {
        self.pages += 1;
        self.chars += text.chars().count();
    }

    pub fn log(&self, fonds: &str, dry_run: bool) {
        tracing::info!(
            fonds,
            dry_run,
            docs_read = self.docs_read,
            docs = self.docs,
            redrawn = self.redrawn,
            pages = self.pages,
            chars = self.chars,
            pdf_bytes = self.pdf_bytes,
            gone = self.gone,
            "ocr: blank pages emptied"
        );
    }
}

/// The pages among `suspects`, counted from 1, of the PDF at `file` that [`blank_pages`] finds
/// blank, `None` when the bucket no longer holds the PDF.
pub async fn redraw(
    bucket: &Bucket,
    file: &str,
    suspects: &[u32],
    tally: &mut Emptied,
) -> Result<Option<BTreeSet<u32>>> {
    let Some(pdf) = bucket.get(file).await? else {
        tracing::warn!(file, "ocr: a kept answer's PDF gone from the bucket");
        tally.gone += 1;
        return Ok(None);
    };
    tally.pdf_bytes += pdf.len();
    tally.redrawn += suspects.len();
    let numbers = suspects.to_vec();
    let subset =
        tokio::task::spawn_blocking(move || lj_sources::pdf::pages_extract(&pdf, &numbers))
            .await
            .map_err(|e| anyhow!("pages task: {e}"))??;
    let blank = blank_pages(&subset, suspects.len()).await?;
    Ok(Some(
        (1..)
            .zip(suspects)
            .filter(|(i, _)| blank.contains_key(i))
            .map(|(_, &n)| n)
            .collect(),
    ))
}

/// The pages kept for the answer at `key`, counted from 1, and whether they are the whole answer
/// rather than pages kept apart.
pub async fn kept_answer(bucket: &Bucket, key: &str) -> Result<(BTreeMap<u32, Value>, bool)> {
    if let Some(pages) = kept::<Vec<Value>>(bucket, key).await? {
        return Ok(((1..).zip(pages).collect(), true));
    }
    let stem = key
        .strip_suffix(".json.zst")
        .ok_or_else(|| anyhow!("{key}: an OCR answer key ends in .json.zst"))?;
    let mut pages = BTreeMap::new();
    for at in bucket.keys(&format!("{stem}/")).await? {
        let Some(n) = at
            .strip_prefix(stem)
            .and_then(|k| k.strip_prefix('/')?.strip_suffix(".json.zst")?.parse().ok())
        else {
            continue;
        };
        let page = kept(bucket, &at)
            .await?
            .ok_or_else(|| anyhow!("{at}: gone from the bucket"))?;
        pages.insert(n, page);
    }
    Ok((pages, false))
}

/// Writes back the answer at `key` [`kept_answer`] read: whole, or its pages `changed` alone.
pub async fn keep_answer(
    bucket: &Bucket,
    key: &str,
    (pages, whole): &(BTreeMap<u32, Value>, bool),
    changed: &BTreeSet<u32>,
) -> Result<()> {
    if *whole {
        return keep(bucket, key, &pages.values().collect::<Vec<_>>()).await;
    }
    let stem = key.trim_end_matches(".json.zst");
    for n in changed {
        keep(bucket, &page_key(stem, *n), &pages[n]).await?;
    }
    Ok(())
}

/// Why `page` is the OCR inventing rather than reading, `None` when it reads.
pub fn invented(page: &str) -> Option<String> {
    if let Some(found) = INVENTED.find(page) {
        return Some(format!("{:?}", found.as_str()));
    }
    let lines: Vec<&str> = page
        .lines()
        .map(str::trim)
        .filter(|line| line.chars().count() >= 8 && line.chars().any(char::is_alphabetic))
        .collect();
    if let Some((line, n)) = most_repeated(lines.iter().copied())
        .filter(|&(line, n)| n >= LOOP_LINES && !heads_entries(&lines, line))
    {
        return Some(format!("{n} times {line:?}"));
    }
    let fractions = FRACTION
        .find_iter(page)
        .map(|f| f.as_str().split_whitespace().collect::<String>());
    most_repeated(fractions)
        .filter(|&(_, n)| n >= LOOP_FRACTIONS)
        .map(|(fraction, n)| format!("{n} times {fraction:?}"))
}

/// [`invented`] over every text of an OCR page, whose markdown only links its tables.
fn invented_page(page: &Value) -> Option<String> {
    let contents = |key: &str| {
        page[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let field = |key: &str| page[key].as_str().unwrap_or_default().to_string();
    [
        field("markdown"),
        field("header"),
        field("footer"),
        contents("tables"),
        contents("blocks"),
    ]
    .iter()
    .find_map(|text| invented(text))
}

/// `line` heads entries each its own, as `# AVIS DE PERTE` heads each notice of a Journal
/// officiel page, rather than loops: most stretches between two of them hold a line the page
/// writes once.
fn heads_entries(lines: &[&str], line: &str) -> bool {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for l in lines {
        *counts.entry(l).or_insert(0) += 1;
    }
    let at: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == line).collect();
    let own = at
        .windows(2)
        .filter(|w| lines[w[0] + 1..w[1]].iter().any(|l| counts[l] == 1))
        .count();
    own * 2 > at.len() - 1
}

fn most_repeated<T: Hash + Eq>(items: impl Iterator<Item = T>) -> Option<(T, usize)> {
    let mut counts = HashMap::new();
    for item in items {
        *counts.entry(item).or_insert(0) += 1;
    }
    counts.into_iter().max_by_key(|&(_, n)| n)
}

/// A page the OCR invented on twice: nothing of what it wrote is the scan's.
#[derive(Debug)]
pub struct Invented(String);

/// A document as [`Ocr::document_kept`] read it: its pages in order, and the numbers, counted
/// from 1, of the pages it invented on twice, absent from `pages`.
pub struct Reading {
    pub pages: Vec<Value>,
    pub refused: Vec<u32>,
}

impl Reading {
    pub fn text(&self) -> String {
        text(&self.pages)
    }
}

impl std::fmt::Display for Invented {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ocr: a page invented twice, {}", self.0)
    }
}

impl std::error::Error for Invented {}

/// An account's sends of the last minute, and until when a 429 holds it back.
#[derive(Default)]
struct Window {
    sent: VecDeque<(Instant, usize)>,
    cooling: Option<Instant>,
}

impl Window {
    /// When `pages` more fit in the account's budget, `now` or before when they fit at once.
    fn free_at(&mut self, pages: usize, now: Instant) -> Instant {
        while self
            .sent
            .front()
            .is_some_and(|&(at, _)| now.duration_since(at) >= MINUTE)
        {
            self.sent.pop_front();
        }
        let used: usize = self.sent.iter().map(|&(_, n)| n).sum();
        let room = match self.sent.front() {
            Some(&(oldest, _)) if used + pages > PAGES_PER_MINUTE => oldest + MINUTE,
            _ => now,
        };
        self.cooling.map_or(room, |cooling| cooling.max(room))
    }
}

/// The account `pages` go to, the first live one they fit in now, its window charged; else the
/// soonest instant one frees, `None` when no account has a live key.
fn pick(
    windows: &mut [Option<&mut Window>],
    pages: usize,
    now: Instant,
) -> Result<usize, Option<Instant>> {
    let mut soonest: Option<Instant> = None;
    for (i, window) in windows.iter_mut().enumerate() {
        let Some(window) = window else { continue };
        let free = window.free_at(pages, now);
        if free <= now {
            window.sent.push_back((now, pages));
            return Ok(i);
        }
        soonest = Some(soonest.map_or(free, |soonest| soonest.min(free)));
    }
    Err(soonest)
}

/// Each account's window, by account, shared by every step the process runs side by side.
static WINDOWS: LazyLock<Mutex<HashMap<String, Window>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The keys of one Mistral account.
struct Account {
    name: String,
    client: MistralClient,
}

pub struct Ocr {
    accounts: Vec<Account>,
}

impl Ocr {
    /// `None` when the `mistral-vibe` pool is empty.
    pub async fn from_pool(repo: &DecisionRepository<'_>) -> Result<Option<Self>> {
        Self::tenants(repo, "").await
    }

    /// The pool's keys whose tenant starts with `prefix`, `None` when it holds none.
    pub async fn tenants(repo: &DecisionRepository<'_>, prefix: &str) -> Result<Option<Self>> {
        let mut keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for credential in repo.credentials("mistral-vibe").await? {
            if !credential.tenant.starts_with(prefix) {
                continue;
            }
            if let Some(key) = credential.api_key() {
                keys.entry(credential.account.clone().unwrap_or_default())
                    .or_default()
                    .push(key.to_string());
            }
        }
        if keys.is_empty() {
            return Ok(None);
        }
        let accounts = keys
            .into_iter()
            .map(|(name, keys)| {
                Ok(Account {
                    name,
                    client: MistralClient::new(keys, "mistral-ocr-latest".to_string())?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Some(Self { accounts }))
    }

    /// Waits until `pages` more fit in an account's budget and returns that account, `None`
    /// once every key was refused.
    async fn admit(&self, pages: usize) -> Option<&Account> {
        loop {
            let picked = {
                let mut windows = WINDOWS.lock().await;
                for account in &self.accounts {
                    windows.entry(account.name.clone()).or_default();
                }
                let mut by_name: HashMap<&str, &mut Window> = windows
                    .iter_mut()
                    .map(|(name, window)| (name.as_str(), window))
                    .collect();
                let mut live: Vec<Option<&mut Window>> = self
                    .accounts
                    .iter()
                    .map(|a| {
                        let window = by_name.remove(a.name.as_str());
                        window.filter(|_| a.client.live_key_count() > 0)
                    })
                    .collect();
                pick(&mut live, pages, Instant::now())
            };
            match picked {
                Ok(i) => return Some(&self.accounts[i]),
                Err(None) => return None,
                Err(Some(at)) => tokio::time::sleep_until(at.into()).await,
            }
        }
    }

    /// Holds `account` back a minute, so that the next call goes to another one.
    async fn cool(&self, account: &Account) {
        if let Some(window) = WINDOWS.lock().await.get_mut(&account.name) {
            window.cooling = Some(Instant::now() + MINUTE);
        }
    }

    /// True once every key of the pool was refused.
    pub fn spent(&self) -> bool {
        self.accounts.iter().all(|a| a.client.live_key_count() == 0)
    }

    /// Parks in the base, until the monthly reset, the keys refused since the last call.
    pub async fn park_spent(&self, repo: &DecisionRepository<'_>) -> Result<()> {
        for account in &self.accounts {
            for (fingerprint, status) in account.client.drain_spent_fingerprints() {
                repo.disable_credential(&fingerprint, None, status as i16, "ocr")
                    .await?;
            }
        }
        Ok(())
    }

    /// The pages of `pdf` as the OCR returns them, each with its blocks and their boxes. A page
    /// the OCR [`invented`] on is read once more, then refused as [`Invented`].
    pub async fn layout(&self, pdf: &[u8], pages: usize, label: &str) -> Result<Vec<Value>> {
        let read = self.layout_pages(pdf, pages, label).await?;
        Ok(read.into_iter().collect::<Result<_, Invented>>()?)
    }

    /// [`Self::layout`], a page the OCR invented on twice refused alone. A [`blank_pages`] page
    /// is not sent: it comes back with no text.
    pub async fn layout_pages(
        &self,
        pdf: &[u8],
        pages: usize,
        label: &str,
    ) -> Result<Vec<Result<Value, Invented>>> {
        let mut blank = blank_pages(pdf, pages).await?;
        let sent: Vec<u32> = (1..=pages as u32)
            .filter(|n| !blank.contains_key(n))
            .collect();
        if !blank.is_empty() {
            tracing::info!(label, pages = blank.len(), "ocr: blank pages left unsent");
        }
        let read = if sent.is_empty() {
            Vec::new()
        } else if sent.len() == pages {
            self.layout_sent(pdf, pages, label).await?
        } else {
            let (file, numbers) = (pdf.to_vec(), sent.clone());
            let subset = tokio::task::spawn_blocking(move || {
                lj_sources::pdf::pages_extract(&file, &numbers)
            })
            .await
            .map_err(|e| anyhow!("pages task: {e}"))??;
            self.layout_sent(&subset, sent.len(), label).await?
        };
        let mut read = read.into_iter();
        Ok((1..=pages as u32)
            .map(|n| match blank.remove(&n) {
                Some(page) => Ok(page),
                None => read.next().expect("one answer per page sent"),
            })
            .collect())
    }

    /// [`Self::layout_pages`] on pages that all go to the OCR.
    async fn layout_sent(
        &self,
        pdf: &[u8],
        pages: usize,
        label: &str,
    ) -> Result<Vec<Result<Value, Invented>>> {
        let read = self.read(pdf, pages, label).await?;
        if read.len() != pages {
            bail!("ocr {label}: {} pages read for {pages}", read.len());
        }
        let again: Vec<u32> = (1..=pages as u32)
            .filter(|&n| invented_page(&read[n as usize - 1]).is_some())
            .collect();
        let mut read: Vec<Result<Value, Invented>> = read.into_iter().map(Ok).collect();
        if again.is_empty() {
            return Ok(read);
        }
        tracing::info!(
            label,
            pages = again.len(),
            "ocr: invented on a page, reading it again"
        );
        let subset = if again.len() == pages {
            pdf.to_vec()
        } else {
            let (file, numbers) = (pdf.to_vec(), again.clone());
            tokio::task::spawn_blocking(move || lj_sources::pdf::pages_extract(&file, &numbers))
                .await
                .map_err(|e| anyhow!("pages task: {e}"))??
        };
        let reread = self.read(&subset, again.len(), label).await?;
        if reread.len() != again.len() {
            bail!(
                "ocr {label}: {} pages read again for {}",
                reread.len(),
                again.len()
            );
        }
        for (n, page) in again.into_iter().zip(reread) {
            read[n as usize - 1] = match invented_page(&page) {
                Some(why) => {
                    tracing::warn!(label, why, "ocr: invented twice on a page, refused");
                    Err(Invented(why))
                }
                None => Ok(page),
            };
        }
        Ok(read)
    }

    async fn read(
        &self,
        pdf: &[u8],
        pages: usize,
        label: &str,
    ) -> Result<Vec<Value>, MistralError> {
        let mut waits = 0;
        loop {
            let Some(account) = self.admit(pages).await else {
                return Err(MistralError::Status(402));
            };
            match ocr_layout_with_retry(&account.client, pdf, label).await {
                Ok(read) => {
                    tracing::info!(label, pages = read.len(), "ocr: pages read");
                    return Ok(read);
                }
                Err(MistralError::Status(429)) if waits < THROTTLE_WAITS * self.accounts.len() => {
                    waits += 1;
                    tracing::warn!(
                        label,
                        waits,
                        "ocr: an account throttled, held back a minute"
                    );
                    self.cool(account).await;
                }
                other => return other,
            }
        }
    }

    /// The [`text`] of the whole of `pdf`.
    pub async fn document(&self, pdf: &[u8], label: &str) -> Result<String> {
        Ok(text(&self.document_layout(pdf, label).await?))
    }

    /// Every page of `pdf`, a page kept beside `key` not sent again; the whole answer is kept at
    /// `key` once no page is missing, a page invented on twice left out for a later call.
    pub async fn document_kept(
        &self,
        bucket: &Bucket,
        key: &str,
        pdf: &[u8],
        label: &str,
    ) -> Result<Reading> {
        let bytes = std::sync::Arc::new(pdf.to_vec());
        let file = bytes.clone();
        let count = tokio::task::spawn_blocking(move || lj_sources::pdf::page_count(&file))
            .await
            .map_err(|e| anyhow!("page count task: {e}"))??;
        let stem = key
            .strip_suffix(".json.zst")
            .ok_or_else(|| anyhow!("{key}: an OCR answer key ends in .json.zst"))?;
        let mut on_bucket: BTreeSet<u32> = bucket
            .keys(stem)
            .await?
            .iter()
            .filter_map(|k| {
                k.strip_prefix(stem)?
                    .strip_prefix('/')?
                    .strip_suffix(".json.zst")?
                    .parse()
                    .ok()
            })
            .collect();
        let missing: Vec<u32> = (1..=count).filter(|n| !on_bucket.contains(n)).collect();
        let batches: Vec<&[u32]> = missing.chunks(BATCH).collect();
        let mut read = BTreeMap::new();
        let mut refused = Vec::new();
        for (i, numbers) in batches.iter().enumerate() {
            let subset = if numbers.len() as u32 == count {
                bytes.to_vec()
            } else {
                let (file, numbers) = (bytes.clone(), numbers.to_vec());
                tokio::task::spawn_blocking(move || lj_sources::pdf::pages_extract(&file, &numbers))
                    .await
                    .map_err(|e| anyhow!("pages task: {e}"))??
            };
            let pages = self.layout_pages(&subset, numbers.len(), label).await?;
            for (&n, page) in numbers.iter().zip(pages) {
                match page {
                    Ok(page) => {
                        read.insert(n, page);
                    }
                    Err(Invented(_)) => refused.push(n),
                }
            }
            if i + 1 < batches.len() {
                keep_pages(bucket, stem, &read, &mut on_bucket).await?;
            }
        }
        if !refused.is_empty() {
            keep_pages(bucket, stem, &read, &mut on_bucket).await?;
        }
        let mut pages = Vec::with_capacity(count as usize);
        for n in 1..=count {
            if let Some(page) = read.remove(&n) {
                pages.push(page);
            } else if on_bucket.contains(&n) {
                let at = page_key(stem, n);
                pages.push(
                    kept(bucket, &at)
                        .await?
                        .ok_or_else(|| anyhow!("{at}: gone from the bucket"))?,
                );
            }
        }
        if refused.is_empty() {
            keep(bucket, key, &pages).await?;
            for n in on_bucket {
                bucket.delete(&page_key(stem, n)).await?;
            }
        } else {
            tracing::warn!(label, refused = ?refused, "ocr: pages invented on twice, the document kept without them");
        }
        Ok(Reading { pages, refused })
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
            read.extend(self.layout(&subset, numbers.len(), label).await?);
        }
        Ok(read)
    }
}

/// The pages of `pdf`, counted from 1, under [`BLANK_INK`], each as the page the OCR returns with
/// no text, sized as its raster.
async fn blank_pages(pdf: &[u8], pages: usize) -> Result<BTreeMap<u32, Value>> {
    let mut child = tokio::process::Command::new("pdftoppm")
        .args(["-r", "75", "-gray", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("pdftoppm: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let feed = async move { stdin.write_all(pdf).await };
    let (fed, out) = tokio::join!(feed, child.wait_with_output());
    let out = out.map_err(|e| anyhow!("pdftoppm: {e}"))?;
    if !out.status.success() {
        bail!("pdftoppm: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    fed.map_err(|e| anyhow!("pdftoppm stdin: {e}"))?;
    let rasters = rasters(&out.stdout)?;
    if rasters.len() != pages {
        bail!("pdftoppm: {} pages drawn for {pages}", rasters.len());
    }
    Ok(rasters
        .into_iter()
        .enumerate()
        .filter(|(_, (_, _, grey))| blank(grey))
        .map(|(i, (width, height, _))| {
            let page = serde_json::json!({
                "index": i,
                "markdown": "",
                "images": [],
                "tables": [],
                "blocks": [],
                "dimensions": { "dpi": 75, "height": height, "width": width },
            });
            (i as u32 + 1, page)
        })
        .collect())
}

fn blank(grey: &[u8]) -> bool {
    grey.iter().filter(|&&v| v < 160).count() as f64 / (grey.len() as f64) < BLANK_INK
}

/// Each page of a `pdftoppm -gray` stream: its width, height and grey samples.
fn rasters(mut stream: &[u8]) -> Result<Vec<(usize, usize, &[u8])>> {
    let mut pages = Vec::new();
    while !stream.is_empty() {
        let fields: Vec<&[u8]> = (0..4)
            .map(|_| field(&mut stream))
            .collect::<Option<_>>()
            .ok_or_else(|| anyhow!("pdftoppm: a truncated header"))?;
        let number = |f: &[u8]| {
            std::str::from_utf8(f)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("pdftoppm: header field {:?}", String::from_utf8_lossy(f)))
        };
        if fields[0] != b"P5" || number(fields[3])? != 255 {
            bail!("pdftoppm: not an 8-bit grey map");
        }
        let (width, height) = (number(fields[1])?, number(fields[2])?);
        // One whitespace byte closes the header.
        let grey = stream
            .get(1..1 + width * height)
            .ok_or_else(|| anyhow!("pdftoppm: a truncated page"))?;
        stream = &stream[1 + width * height..];
        pages.push((width, height, grey));
    }
    Ok(pages)
}

fn field<'a>(stream: &mut &'a [u8]) -> Option<&'a [u8]> {
    let start = stream.iter().position(|b| !b.is_ascii_whitespace())?;
    let rest = &stream[start..];
    let end = rest
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(rest.len());
    *stream = &rest[end..];
    Some(&rest[..end])
}

/// Page `page` of the answer at `<stem>.json.zst`, kept apart until the whole answer is.
pub fn page_key(stem: &str, page: u32) -> String {
    format!("{stem}/{page}.json.zst")
}

/// Keeps each page of `read` not on the bucket yet.
async fn keep_pages(
    bucket: &Bucket,
    stem: &str,
    read: &BTreeMap<u32, Value>,
    on_bucket: &mut BTreeSet<u32>,
) -> Result<()> {
    for (&n, page) in read {
        if on_bucket.insert(n) {
            keep(bucket, &page_key(stem, n), page).await?;
        }
    }
    Ok(())
}

/// Keeps Mistral's `answer` at `key`, its JSON compressed by zstd.
pub async fn keep(bucket: &Bucket, key: &str, answer: &impl Serialize) -> Result<()> {
    let json = serde_json::to_vec(answer)?;
    let packed = zstd::encode_all(json.as_slice(), zstd::DEFAULT_COMPRESSION_LEVEL)?;
    bucket.put(key, packed).await?;
    Ok(())
}

/// The answer [`keep`] left at `key`, `None` when there is none.
pub async fn kept<T: DeserializeOwned>(bucket: &Bucket, key: &str) -> Result<Option<T>> {
    let Some(packed) = bucket.get(key).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&zstd::decode_all(&*packed)?)?))
}

/// The text of a document's pages, each page's [`markdown`] joined by a line break.
pub fn text(pages: &[Value]) -> String {
    pages.iter().map(markdown).collect::<Vec<_>>().join("\n")
}

/// The page's markdown, each table in place of the link the OCR leaves to it.
pub fn markdown(page: &Value) -> String {
    let mut text = page["markdown"].as_str().unwrap_or_default().to_string();
    for table in page["tables"].as_array().into_iter().flatten() {
        if let (Some(id), Some(content)) = (table["id"].as_str(), table["content"].as_str()) {
            text = text.replace(&format!("[{id}]({id})"), content);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        blank, invented, invented_page, markdown, pick, rasters, Window, MINUTE, PAGES_PER_MINUTE,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn pages_go_to_an_account_with_room_and_wait_only_when_none_has() {
        let now = Instant::now();
        let mut throttled = Window {
            cooling: Some(now + MINUTE),
            ..Default::default()
        };
        let mut full = Window::default();
        full.sent
            .push_back((now - Duration::from_secs(20), PAGES_PER_MINUTE - 10));
        let mut free = Window::default();
        assert_eq!(
            pick(
                &mut [None, Some(&mut throttled), Some(&mut full), Some(&mut free)],
                16,
                now
            ),
            Ok(3)
        );
        assert_eq!(pick(&mut [Some(&mut full)], 10, now), Ok(0));
        assert_eq!(
            pick(&mut [Some(&mut throttled), Some(&mut full)], 16, now),
            Err(Some(now + Duration::from_secs(40)))
        );
        assert_eq!(pick(&mut [None, None], 16, now), Err(None));
    }

    #[test]
    fn a_table_takes_the_place_of_its_link() {
        let page = serde_json::json!({
            "markdown": "ANNEXE\n\n[tbl-0.md](tbl-0.md)\n\nFait à Paris\n\n[tbl-1.md](tbl-1.md)",
            "tables": [
                {"id": "tbl-0.md", "content": "| Grade | Indice |\n| --- | --- |\n| Attaché | 434 |"},
                {"id": "tbl-1.md", "content": "| Visa |\n| --- |"},
            ],
        });
        assert_eq!(
            markdown(&page),
            "ANNEXE\n\n| Grade | Indice |\n| --- | --- |\n| Attaché | 434 |\n\nFait à Paris\n\n| Visa |\n| --- |"
        );
    }

    #[test]
    fn a_table_the_markdown_only_links_is_read_too() {
        let page = serde_json::json!({
            "markdown": "SOMMARIO\n\n[tbl-0.md](tbl-0.md)",
            "tables": [{"id": "tbl-0.md", "content": "| 项目名称 | 工程内容 |\n| --- | --- |"}],
        });
        assert!(invented_page(&page).is_some());
    }

    #[test]
    fn a_page_the_ocr_invented_on_is_caught() {
        let numbers: String = (1..=35).map(|n| format!("{n}. ")).collect();
        let fractions: String = ('a'..='j')
            .map(|c| format!("\\( \\frac{{1}}{{2}} \\) ({c}) "))
            .collect();
        for page in [
            "Pour le président :\n\\( \\therefore m = \\frac{3}{11} \\)".to_string(),
            "Fait à Paris,\n\n[Non-Text]\n\n[Non-Text]".to_string(),
            "en décembre 2017 ont permis de很好地 faire apparaitre les raisons".to_string(),
            "1. 2017年1月1日".to_string(),
            "The following table provides the information in a specific format:".to_string(),
            "The Ground Truth image displays a single, solid horizontal line.".to_string(),
            "The image contains no text.".to_string(),
            "Niort, le 30 octobre[{\"box_2d\": [687, 345, 919, 371], \"label\": \"text\""
                .to_string(),
            format!("Le ministre,\n{numbers}"),
            "Permettez avec \"état parcellation\"\n".repeat(40),
            format!(
                "# AVIS DE PERTE\nIl a été perdu le titre n° 1.\n{}",
                "# AVIS DE PERTE\nIl a été perdu le titre n° 2.\n".repeat(31)
            ),
            format!("2. {fractions}"),
        ] {
            assert!(invented(&page).is_some(), "{page}");
        }
    }

    #[test]
    fn a_page_read_passes() {
        for page in [
            "Le calcul du ratio est le suivant :\n\n$$\\frac{\\text{Fonds de roulement} \\times 360}{\\text{Total charges}}$$".to_string(),
            "### Définition du TRGE\n$$TRGE = \\frac{C4}{C2+C3+C4}$$".to_string(),
            "selon la formule : $$C = \\frac{1}{2} \\sqrt{V}$$ Avec V: le volume en m³".to_string(),
            "$$K = 1.20 \\frac{2,5 a_g S}{q g} = \\frac{3 a_g S}{q g}$$\n".repeat(2),
            "| LES DEUX | oui |\n".repeat(22),
            "__________\n".repeat(36),
            "1. Considérant que la requête est recevable ;\n2. Considérant que le moyen manque en fait ;".to_string(),
            (1..=32)
                .map(|n| format!("# AVIS DE PERTE\nIl a été perdu le titre foncier n° {n} de Dakar.\n"))
                .collect(),
        ] {
            assert_eq!(invented(&page), None, "{page}");
        }
    }

    #[test]
    fn a_page_under_a_thousandth_of_ink_is_blank_and_each_page_of_the_stream_is_read() {
        let page = |dark: usize| {
            let mut grey = vec![255u8; 100 * 100];
            grey[..dark].fill(0);
            let mut bytes = b"P5\n100 100\n255\n".to_vec();
            bytes.extend(grey);
            bytes
        };
        let stream = [page(9), page(10), page(0)].concat();
        let pages = rasters(&stream).unwrap();
        assert_eq!(pages.len(), 3);
        assert!(pages.iter().all(|&(w, h, _)| (w, h) == (100, 100)));
        let blanks: Vec<bool> = pages.iter().map(|(_, _, grey)| blank(grey)).collect();
        assert_eq!(blanks, [true, false, true]);
    }
}
