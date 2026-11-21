//! A Flemish act's history on the Codex, its states walked back from today.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_parse::CodexVersion;
use lj_sources::codex::CodexClient;

use super::super::legal_corpus::{CorpusArticle, CorpusVersion};

/// What one act's walk downloads at most; VLAREM II serves tens of MB a state.
const BUDGET_STATES: usize = 60;
const BUDGET_BYTES: usize = 300 << 20;

/// An act as its latest state names it, with every article version its states served.
pub(super) struct Walked {
    pub adopted: Option<String>,
    pub published: Option<String>,
    pub checksum: u64,
    pub requests: usize,
    /// The earliest day read, when the budget stopped the walk before the act's first state.
    pub cut: Option<String>,
    versions: Vec<CodexVersion>,
}

fn cached_path(cache: &Path, id: u64, day: &str) -> PathBuf {
    cache.join(id.to_string()).join(format!("{day}.json.zst"))
}

/// A dated state read before, kept compressed in the ingest cache.
fn cached(cache: &Path, id: u64, day: &str) -> Result<Option<String>> {
    match std::fs::read(cached_path(cache, id, day)) {
        Ok(bytes) => Ok(Some(String::from_utf8(zstd::decode_all(&bytes[..])?)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn keep(cache: &Path, id: u64, day: &str, payload: &str) -> Result<()> {
    let path = cached_path(cache, id, day);
    std::fs::create_dir_all(path.parent().expect("a cached state has a folder"))?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, zstd::encode_all(payload.as_bytes(), 3)?)?;
    std::fs::rename(&partial, &path)?;
    Ok(())
}

/// The latest dated state of `id` the cache holds.
pub(super) fn cached_latest(cache: &Path, id: u64) -> Result<Option<lj_parse::CodexState>> {
    let latest = match std::fs::read_dir(cache.join(id.to_string())) {
        Ok(entries) => entries
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<String>>>()?
            .into_iter()
            .filter_map(|name| name.strip_suffix(".json.zst").map(str::to_string))
            .max(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    latest
        .map(|day| {
            let payload = cached(cache, id, &day)?
                .ok_or_else(|| anyhow!("codex {id}: {day} left the cache"))?;
            lj_parse::codex_state(&payload).map_err(|e| anyhow!("codex {id} {day}: {e}"))
        })
        .transpose()
}

/// Reads today's state, then the day before the latest version start no state read yet explains,
/// a dated state from the cache when it holds it, until the budget runs out.
pub(super) async fn walk(client: &CodexClient, id: u64, cache: &Path) -> Result<Walked> {
    let mut by_id: HashMap<u64, CodexVersion> = HashMap::new();
    let mut order: Vec<u64> = Vec::new();
    let (mut starts, mut walked) = (BTreeSet::new(), HashSet::new());
    let mut head: Option<(Option<String>, Option<String>, u64)> = None;
    let mut day: Option<String> = None;
    let (mut requests, mut downloaded) = (0, 0);
    let mut cut = None;
    loop {
        let payload = match day.as_deref().map(|d| cached(cache, id, d)).transpose()? {
            Some(Some(payload)) => payload,
            _ => {
                let payload = client.state(id, day.as_deref()).await?;
                if let Some(d) = &day {
                    keep(cache, id, d, &payload)?;
                }
                requests += 1;
                downloaded += payload.len();
                payload
            }
        };
        let state = lj_parse::codex_state(&payload).map_err(|e| anyhow!("codex {id}: {e}"))?;
        if head.is_none() {
            let checksum = xxhash_rust::xxh3::xxh3_64(format!("{:?}", state.versions).as_bytes());
            head = Some((state.adopted, state.published, checksum));
        }
        for v in state.versions {
            if let Some(start) = &v.start {
                starts.insert(start.clone());
            }
            if !by_id.contains_key(&v.id) {
                order.push(v.id);
            }
            by_id.entry(v.id).or_insert(v);
        }
        let Some(next) = starts.iter().rev().find(|s| !walked.contains(*s)).cloned() else {
            break;
        };
        if requests >= BUDGET_STATES || downloaded >= BUDGET_BYTES {
            cut = Some(
                day.clone()
                    .unwrap_or_else(|| chrono::Utc::now().date_naive().to_string()),
            );
            break;
        }
        let before = NaiveDate::parse_from_str(&next, "%Y-%m-%d")? - chrono::Duration::days(1);
        walked.insert(next);
        day = Some(before.to_string());
    }
    let (adopted, published, checksum) = head.expect("the loop reads the current state first");
    Ok(Walked {
        adopted,
        published,
        checksum,
        requests,
        cut,
        versions: order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect(),
    })
}

/// A version that says only that its article is gone: `...`, `Opgeheven`.
fn repealed(texte: &str) -> bool {
    let t = texte.trim().trim_matches(&['[', ']', ' '][..]);
    t.is_empty()
        || t.chars().all(|c| matches!(c, '.' | '…' | ' '))
        || t.to_lowercase().starts_with("opgeheven")
}

fn next_day(day: &str) -> Result<String> {
    Ok((NaiveDate::parse_from_str(day, "%Y-%m-%d")? + chrono::Duration::days(1)).to_string())
}

impl Walked {
    /// Each article in the order the states first showed it, a version ending at the next start.
    pub fn articles(self) -> Result<Vec<CorpusArticle>> {
        let mut order: Vec<String> = Vec::new();
        let mut by_num: HashMap<String, Vec<CodexVersion>> = HashMap::new();
        for v in self.versions {
            if !by_num.contains_key(&v.num) {
                order.push(v.num.clone());
            }
            by_num.entry(v.num.clone()).or_default().push(v);
        }
        order
            .into_iter()
            .map(|num| {
                let mut versions = by_num.remove(&num).unwrap_or_default();
                versions.sort_by(|a, b| (&a.start, a.id).cmp(&(&b.start, b.id)));
                versions.dedup_by(|later, earlier| {
                    let same = later.start == earlier.start;
                    if same {
                        std::mem::swap(later, earlier);
                    }
                    same
                });
                let title_path = versions.iter().rev().find_map(|v| v.title_path.clone());
                let starts: Vec<Option<String>> =
                    versions.iter().skip(1).map(|v| v.start.clone()).collect();
                let corpus = versions
                    .into_iter()
                    .zip(starts.into_iter().map(Some).chain([None]))
                    .map(|(v, next)| {
                        let end = v.end.as_deref().map(next_day).transpose()?;
                        let end = match (end, next.flatten()) {
                            (Some(end), Some(next)) => Some(end.min(next)),
                            (end, next) => next.or(end),
                        };
                        Ok(CorpusVersion {
                            date_debut: v.start,
                            date_fin: end,
                            status: if repealed(&v.texte) {
                                "ABROGE"
                            } else {
                                "VIGUEUR"
                            }
                            .to_string(),
                            texte: v.texte,
                            source: None,
                            source_url: None,
                            source_asof: None,
                            source_upstream_url: None,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(CorpusArticle {
                    num,
                    num_key: None,
                    title_path,
                    nota: None,
                    texte: None,
                    texte_original: None,
                    lang_original: None,
                    translation: None,
                    versions: Some(corpus),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(
        id: u64,
        num: &str,
        start: Option<&str>,
        end: Option<&str>,
        texte: &str,
    ) -> CodexVersion {
        CodexVersion {
            id,
            num: num.to_string(),
            title_path: None,
            texte: texte.to_string(),
            start: start.map(str::to_string),
            end: end.map(str::to_string),
        }
    }

    #[test]
    fn a_version_ends_the_day_after_its_last_and_never_past_the_next_start() {
        let walked = Walked {
            adopted: None,
            published: None,
            checksum: 0,
            requests: 3,
            cut: None,
            versions: vec![
                version(3, "10.1.1", Some("2019-01-01"), None, "C"),
                version(1, "10.1.1", None, Some("2012-05-31"), "A"),
                version(2, "10.1.1", Some("2012-06-01"), None, "B"),
                version(4, "10.1.2", Some("2012-06-01"), Some("2018-12-31"), "..."),
            ],
        };
        let arts = walked.articles().unwrap();
        let windows: Vec<_> = arts[0]
            .versions
            .as_ref()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v.date_debut.as_deref(),
                    v.date_fin.as_deref(),
                    v.texte.as_str(),
                )
            })
            .collect();
        assert_eq!(
            windows,
            vec![
                (None, Some("2012-06-01"), "A"),
                (Some("2012-06-01"), Some("2019-01-01"), "B"),
                (Some("2019-01-01"), None, "C"),
            ]
        );
        let repealed = &arts[1].versions.as_ref().unwrap()[0];
        assert_eq!(repealed.date_fin.as_deref(), Some("2019-01-01"));
        assert_eq!(repealed.status, "ABROGE");
    }
}
