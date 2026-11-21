//! Morocco's texts as the Adala portal holds them: one version each, the consolidation its PDF
//! names, its French text and its Arabic one.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{anyhow, Result};
use lj_norm::text_aliases::{self, TextAlias};
use lj_parse::{AdalaFile, AdalaIdentity, ConsolidatedArticle};
use lj_sources::adala::AdalaClient;
use lj_store::repository::{ArticleLanguageWrite, DecisionRepository};
use regex::Regex;

use super::super::legal_corpus::{load_doc, write_aliases, CorpusArticle, CorpusDoc};
use super::{refused, Run, Tally};

pub(super) const SOURCE: &str = "adala";
/// Fewer characters a page than this, and a PDF is a scan.
const NATIVE_CHARS_PER_PAGE: usize = 200;
static DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\((1er|\d{1,2})\s+(\p{L}+)\s+(\d{4})\)").unwrap());

fn uid(identity: &AdalaIdentity) -> String {
    match &identity.number {
        Some(n) => format!("MA/{}-{}", identity.code, lj_parse::journal_number_key(n)),
        None => format!("MA/{}", identity.code),
    }
}

fn checksum(f: &AdalaFile) -> u64 {
    xxhash_rust::xxh3::xxh3_64(format!("{}|{}|{:?}", f.name, f.path, f.updated).as_bytes())
}

/// The Gregorian day a Moroccan act gives after its Hegirian one, `(24 mai 2022)`.
fn day(s: &str) -> Option<String> {
    let c = DAY.captures(s)?;
    lj_core::dates::parse_date_fr(&format!("{} {} {}", &c[1], &c[2], &c[3])).map(|d| d.to_string())
}

fn article(
    num: String,
    num_key: Option<String>,
    texte: &str,
    path: Option<String>,
) -> CorpusArticle {
    CorpusArticle {
        num,
        num_key,
        texte: Some(lj_core::normalizer::normalize_chars(texte)),
        texte_original: None,
        lang_original: None,
        translation: None,
        nota: None,
        versions: None,
        title_path: path,
    }
}

fn articles(parsed: Vec<ConsolidatedArticle>, body: impl FnOnce() -> String) -> Vec<CorpusArticle> {
    match parsed.is_empty() {
        false => parsed
            .into_iter()
            .map(|a| article(a.num, None, &a.texte, a.title_path))
            .collect(),
        true => vec![article(
            String::new(),
            Some("body".to_string()),
            &body(),
            None,
        )],
    }
}

/// An Arabic file's act, its text in Arabic and its days read as a French file's are.
fn arabic_doc(f: &AdalaFile, identity: &AdalaIdentity, url: &str, text: &str) -> CorpusDoc {
    let head: String = text.chars().take(1500).collect();
    let (law, signed) = match lj_parse::adala_arabic_promulgation(text) {
        Some((at, signed)) => (&text[at..], Some(signed)),
        None => (text, None),
    };
    let date_texte = lj_parse::adala_arabic_day(&f.name)
        .or_else(|| signed.and_then(lj_parse::adala_arabic_day))
        .or_else(|| lj_parse::adala_arabic_day(&head));
    CorpusDoc {
        text_uid: uid(identity),
        source: SOURCE.to_string(),
        jurisdiction: "MA".to_string(),
        title: f.name.clone(),
        title_key: None,
        nature: identity.nature.to_string(),
        translation: "officiel".to_string(),
        source_url: Some(url.to_string()),
        source_asof: None,
        source_upstream_url: None,
        effect_date: lj_parse::adala_arabic_consolidated(text).or_else(|| date_texte.clone()),
        date_texte,
        detect_abrogation: false,
        num_prefix_agnostic: false,
        articles: articles(lj_parse::adala_arabic_articles(law), || {
            lj_parse::adala_arabic_body(law)
        }),
        links: Vec::new(),
    }
}

/// A file's text cached under its checksum, the first line the checksum; an empty text is a scan.
fn cached(cache: &Path, f: &AdalaFile) -> Result<Option<String>> {
    match std::fs::read(cache.join(format!("{}.txt.zst", f.id))) {
        Ok(bytes) => {
            let text = String::from_utf8(zstd::decode_all(&bytes[..])?)?;
            let (head, body) = text.split_once('\n').unwrap_or((&text, ""));
            Ok((head == format!("{:016x}", checksum(f))).then(|| body.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn cache(cache: &Path, f: &AdalaFile, text: &str) -> Result<()> {
    std::fs::create_dir_all(cache)?;
    let path = cache.join(format!("{}.txt.zst", f.id));
    let partial = path.with_extension("partial");
    let content = format!("{:016x}\n{text}", checksum(f));
    std::fs::write(&partial, zstd::encode_all(content.as_bytes(), 3)?)?;
    std::fs::rename(&partial, &path)?;
    Ok(())
}

fn doc(f: &AdalaFile, identity: &AdalaIdentity, url: &str, text: &str) -> CorpusDoc {
    let head: String = text.chars().take(1500).collect();
    let (law, signed) = match lj_parse::adala_promulgation(text) {
        Some((at, signed)) => (&text[at..], Some(signed)),
        None => (text, None),
    };
    let articles = articles(lj_parse::journal_text_articles(law), || {
        lj_parse::journal_text_body(law)
    });
    let date_texte = day(&f.name)
        .or_else(|| signed.and_then(day))
        .or_else(|| day(&head));
    CorpusDoc {
        text_uid: uid(identity),
        source: SOURCE.to_string(),
        jurisdiction: "MA".to_string(),
        title: f.name.clone(),
        title_key: None,
        nature: identity.nature.to_string(),
        translation: "officiel".to_string(),
        source_url: Some(url.to_string()),
        source_asof: None,
        source_upstream_url: None,
        effect_date: lj_parse::adala_consolidated(text).or_else(|| date_texte.clone()),
        date_texte,
        detect_abrogation: false,
        num_prefix_agnostic: false,
        articles,
        links: Vec::new(),
    }
}

/// The Arabic text of each version in force of a text the base holds, matched on its article.
fn hang(
    versions: &[(i64, String, Option<chrono::NaiveDate>)],
    articles: &[CorpusArticle],
    checksum: u64,
) -> Vec<ArticleLanguageWrite> {
    let keyed: HashMap<String, &CorpusArticle> = articles
        .iter()
        .map(|a| (lj_norm::article_key::identity_key(&a.num), a))
        .collect();
    let mut rows: Vec<ArticleLanguageWrite> = versions
        .iter()
        .filter(|(_, _, fin)| fin.is_none())
        .filter_map(|(id, num, _)| {
            let a = keyed.get(&lj_norm::article_key::identity_key(num))?;
            Some(ArticleLanguageWrite {
                article_id: *id,
                text_status: "official".to_string(),
                texte: a.texte.clone()?,
                title_path: a.title_path.clone(),
                content_checksum: i64::from_ne_bytes(checksum.to_ne_bytes()),
            })
        })
        .collect();
    rows.sort_by_key(|r| r.article_id);
    rows
}

impl Run<'_> {
    pub(super) async fn adala_lane(&self, client: &AdalaClient, limit: Option<usize>) -> Tally {
        let mut tally = Tally::default();
        if let Err(e) = self.adala(client, limit, &mut tally).await {
            tracing::error!(publisher = self.source, error = %e, "Adala run stopped");
            tally.failed.push("adala".to_string());
        }
        if let Err(e) = self.adala_arabic(client, limit, &mut tally).await {
            tracing::error!(publisher = self.source, error = %e, "Adala Arabic run stopped");
            tally.failed.push("adala-ar".to_string());
        }
        tally
    }

    /// Reads Adala's Arabic files: an act the base holds in French takes the Arabic text of each
    /// of its articles in force, an act it does not hold is written in Arabic. A file's text is
    /// kept in the cache, so an act is hung again without a request when its French changes.
    async fn adala_arabic(
        &self,
        client: &AdalaClient,
        limit: Option<usize>,
        tally: &mut Tally,
    ) -> Result<()> {
        let mut files = client.files("AR").await?;
        files.sort_by_key(|f| std::cmp::Reverse(f.id));
        if let Some(limit) = limit {
            files.truncate(limit);
        }
        let cache_dir = self.cache.join("adala").join("ar");
        let (mut scans, mut unkeyed, mut twice, mut left, mut fetched) = (0, 0, 0, 0, 0);
        let mut acts: HashMap<String, (&AdalaFile, AdalaIdentity, String)> = HashMap::new();
        for f in &files {
            let text = match cached(&cache_dir, f)? {
                Some(text) => text,
                None if self
                    .deadline
                    .is_some_and(|d| std::time::Instant::now() >= d) =>
                {
                    left += 1;
                    continue;
                }
                None => {
                    let url = AdalaClient::pdf_url(&f.path);
                    let read = async {
                        let bytes = client.pdf(&url).await?;
                        let pages = lj_sources::pdf::page_count(&bytes)
                            .map_err(|e| anyhow!("{url}: {e}"))?;
                        let text =
                            lj_sources::rtl::rtl_text(&bytes).map_err(|e| anyhow!("{url}: {e}"))?;
                        anyhow::Ok((pages, lj_parse::adala_arabic_text(&text)))
                    };
                    let (pages, text) = match read.await {
                        Ok(read) => read,
                        Err(e) => {
                            tracing::error!(publisher = self.source, id = f.id, error = %e, "followed text failed");
                            tally.failed.push(format!("ar:{}", f.id));
                            if refused(&e) {
                                break;
                            }
                            continue;
                        }
                    };
                    fetched += 1;
                    let chars: usize = text.split_whitespace().map(str::len).sum();
                    let text = match chars < NATIVE_CHARS_PER_PAGE * pages.max(1) as usize {
                        true => String::new(),
                        false => text,
                    };
                    cache(&cache_dir, f, &text)?;
                    text
                }
            };
            if text.is_empty() {
                scans += 1;
                continue;
            }
            let Some(identity) = lj_parse::adala_arabic_identity(&f.name, &f.path, &text) else {
                tracing::warn!(
                    publisher = self.source,
                    id = f.id,
                    name = f.name,
                    "file names no act"
                );
                unkeyed += 1;
                continue;
            };
            let key = uid(&identity);
            if acts.contains_key(&key) {
                twice += 1;
                continue;
            }
            acts.insert(key, (f, identity, text));
        }
        tracing::info!(
            publisher = self.source,
            listed = files.len(),
            fetched,
            acts = acts.len(),
            "Adala Arabic files read"
        );
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let (mut hung, mut written, mut articles_hung) = (0usize, 0usize, 0usize);
        let mut visited: Vec<(String, u64)> = Vec::new();
        for (key, (f, identity, text)) in &acts {
            let url = AdalaClient::pdf_url(&f.path);
            let doc = arabic_doc(f, identity, &url, text);
            let languages = repo
                .texte_languages(key)
                .await
                .map_err(|e| anyhow!("texte_languages {key}: {e}"))?;
            let french = languages.iter().any(|l| l != "ar");
            let item = format!("ar:{key}");
            let load = async {
                if french {
                    let versions = repo
                        .article_version_ends(key)
                        .await
                        .map_err(|e| anyhow!("article_version_ends {key}: {e}"))?;
                    let rows = hang(&versions, &doc.articles, checksum(f));
                    let ids: Vec<i64> = rows.iter().map(|r| r.article_id).collect();
                    let visit = xxhash_rust::xxh3::xxh3_64(
                        format!("{}{key}{ids:?}", checksum(f)).as_bytes(),
                    );
                    if self
                        .visits
                        .get(&item)
                        .is_some_and(|v| v.content_checksum == visit)
                    {
                        return anyhow::Ok(None);
                    }
                    if self.dry_run {
                        let in_force = versions.iter().filter(|v| v.2.is_none()).count();
                        tracing::info!(
                            publisher = self.source,
                            key = key.as_str(),
                            articles = doc.articles.len(),
                            in_force,
                            hung = rows.len(),
                            "Adala Arabic text hung"
                        );
                    } else {
                        repo.replace_language_texts(key, "ar", &rows)
                            .await
                            .map_err(|e| anyhow!("replace_language_texts {key}: {e}"))?;
                    }
                    return anyhow::Ok(Some((visit, rows.len())));
                }
                let visit =
                    xxhash_rust::xxh3::xxh3_64(format!("{}{key}texte", checksum(f)).as_bytes());
                if self
                    .visits
                    .get(&item)
                    .is_some_and(|v| v.content_checksum == visit)
                {
                    return anyhow::Ok(None);
                }
                if self.dry_run {
                    tracing::info!(publisher = self.source, key = key.as_str(), articles = doc.articles.len(), effect = ?doc.effect_date, "Adala Arabic text read");
                } else {
                    load_doc(
                        &repo,
                        &doc,
                        self.read_on,
                        self.source,
                        &mut tally.publie_targets,
                    )
                    .await?;
                    repo.set_texte_language(key, "ar")
                        .await
                        .map_err(|e| anyhow!("set_texte_language {key}: {e}"))?;
                    let aliases = [
                        text_aliases::of(key),
                        identity
                            .number
                            .iter()
                            .map(|n| TextAlias::number(n))
                            .collect(),
                    ]
                    .concat();
                    write_aliases(&repo, key, &aliases).await?;
                }
                anyhow::Ok(Some((visit, 0)))
            };
            match load.await {
                Ok(None) => tally.unchanged += 1,
                Ok(Some((visit, n))) => {
                    match french {
                        true => {
                            hung += 1;
                            articles_hung += n;
                        }
                        false => written += 1,
                    }
                    tally.written += 1;
                    visited.push((item, visit));
                }
                Err(e) => {
                    tracing::error!(publisher = self.source, key = key.as_str(), error = %e, "followed text failed");
                    tally.failed.push(item);
                }
            }
            if visited.len() >= 100 && !self.dry_run {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
                visited.clear();
            }
        }
        if !self.dry_run {
            repo.put_norm_visits(self.source, &visited)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        self.budget_spent("adala-ar", left);
        tracing::info!(
            publisher = self.source,
            hung,
            articles_hung,
            written,
            scans,
            unkeyed,
            twice,
            "Adala Arabic acts"
        );
        Ok(())
    }

    async fn adala(
        &self,
        client: &AdalaClient,
        limit: Option<usize>,
        tally: &mut Tally,
    ) -> Result<()> {
        let mut files = client.files("FR").await?;
        files.sort_by_key(|f| std::cmp::Reverse(f.id));
        let listed = files.len();
        files.retain(|f| {
            let unchanged = self
                .visits
                .get(&f.id.to_string())
                .is_some_and(|v| v.content_checksum == checksum(f));
            tally.unchanged += usize::from(unchanged);
            !unchanged
        });
        if let Some(limit) = limit {
            files.truncate(limit);
        }
        tracing::info!(
            publisher = self.source,
            listed,
            to_read = files.len(),
            "Adala files listed"
        );
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let (mut scans, mut unkeyed, mut twice) = (0usize, 0usize, 0usize);
        let mut written: HashMap<String, u64> = HashMap::new();
        let mut visited: Vec<(String, u64)> = Vec::new();
        let mut left = 0usize;
        for f in &files {
            if self.left_for_later(&f.id.to_string()) {
                left += 1;
                continue;
            }
            tally.listed += 1;
            let url = AdalaClient::pdf_url(&f.path);
            let read = async {
                let bytes = client.pdf(&url).await?;
                let pages =
                    lj_sources::pdf::page_count(&bytes).map_err(|e| anyhow!("{url}: {e}"))?;
                let text = lj_sources::pdf::pdftotext_extract(&bytes)
                    .map_err(|e| anyhow!("{url}: {e}"))?;
                anyhow::Ok((pages, text))
            };
            let (pages, text) = match read.await {
                Ok(read) => read,
                Err(e) => {
                    tracing::error!(publisher = self.source, id = f.id, error = %e, "followed text failed");
                    tally.failed.push(f.id.to_string());
                    if refused(&e) {
                        break;
                    }
                    continue;
                }
            };
            let visit = (f.id.to_string(), checksum(f));
            let chars: usize = text.split_whitespace().map(str::len).sum();
            if chars < NATIVE_CHARS_PER_PAGE * pages.max(1) as usize {
                scans += 1;
                visited.push(visit);
                continue;
            }
            let Some(identity) = lj_parse::adala_identity(&f.name, &text) else {
                tracing::warn!(
                    publisher = self.source,
                    id = f.id,
                    name = f.name,
                    "file names no act"
                );
                unkeyed += 1;
                visited.push(visit);
                continue;
            };
            let doc = doc(f, &identity, &url, &text);
            if let Some(other) = written.get(&doc.text_uid) {
                tracing::warn!(
                    publisher = self.source,
                    id = f.id,
                    other,
                    key = doc.text_uid,
                    "a newer file holds this text"
                );
                twice += 1;
                visited.push(visit);
                continue;
            }
            written.insert(doc.text_uid.clone(), f.id);
            if self.dry_run {
                tracing::info!(publisher = self.source, id = f.id, key = doc.text_uid, effect = ?doc.effect_date, articles = doc.articles.len(), "Adala text read");
                continue;
            }
            let aliases = [
                text_aliases::of(&doc.text_uid),
                identity
                    .number
                    .iter()
                    .map(|n| TextAlias::number(n))
                    .collect(),
            ]
            .concat();
            let load = async {
                load_doc(
                    &repo,
                    &doc,
                    self.read_on,
                    self.source,
                    &mut tally.publie_targets,
                )
                .await?;
                write_aliases(&repo, &doc.text_uid, &aliases).await
            };
            match load.await {
                Ok(_) => {
                    tally.written += 1;
                    visited.push(visit);
                }
                Err(e) => {
                    tracing::error!(publisher = self.source, key = doc.text_uid, error = %e, "followed text failed");
                    tally.failed.push(doc.text_uid);
                }
            }
            if visited.len() >= 100 {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
                visited.clear();
            }
        }
        if !self.dry_run {
            repo.put_norm_visits(self.source, &visited)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        self.budget_spent(self.jurisdiction, left);
        if scans + unkeyed + twice > 0 {
            tracing::warn!(
                publisher = self.source,
                scans,
                unkeyed,
                twice,
                "files visited and not written"
            );
        }
        Ok(())
    }
}
