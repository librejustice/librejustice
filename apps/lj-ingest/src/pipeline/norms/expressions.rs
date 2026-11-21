//! A federal act's texts in its other official languages, hung on the versions its French text
//! cut: German and Italian at Fedlex, English at Justice Laws.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::NaiveDate;
use lj_parse::ConsolidatedArticle;

pub(super) const LANGUAGES: [&str; 2] = ["de", "it"];

/// The visit a language's states of an act are kept under, its days as checksum.
pub(super) fn text_item(key: &str, language: &str) -> String {
    format!("{key}#{language}")
}

/// The visit one state of an act in a language is kept under, its file as checksum.
pub(super) fn state_item(key: &str, day: NaiveDate, language: &str) -> String {
    format!("{key}@{day}#{language}")
}

fn cached_path(
    cache: &Path,
    publisher: &str,
    key: &str,
    day: NaiveDate,
    language: &str,
) -> PathBuf {
    cache
        .join(publisher)
        .join(key)
        .join(format!("{day}.{language}.xml.zst"))
}

pub(super) fn cached(
    cache: &Path,
    publisher: &str,
    key: &str,
    day: NaiveDate,
    language: &str,
) -> Result<Option<String>> {
    match std::fs::read(cached_path(cache, publisher, key, day, language)) {
        Ok(bytes) => Ok(Some(String::from_utf8(zstd::decode_all(&bytes[..])?)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The day a state's file was kept, `None` when none is.
pub(super) fn kept_on(
    cache: &Path,
    publisher: &str,
    key: &str,
    day: NaiveDate,
    language: &str,
) -> Result<Option<NaiveDate>> {
    match std::fs::metadata(cached_path(cache, publisher, key, day, language)) {
        Ok(m) => Ok(Some(
            chrono::DateTime::<chrono::Utc>::from(m.modified()?).date_naive(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn cache(
    cache: &Path,
    publisher: &str,
    key: &str,
    day: NaiveDate,
    language: &str,
    xml: &str,
) -> Result<()> {
    let path = cached_path(cache, publisher, key, day, language);
    std::fs::create_dir_all(path.parent().expect("a cached state has a folder"))?;
    let partial = path.with_extension("partial");
    std::fs::write(&partial, zstd::encode_all(xml.as_bytes(), 3)?)?;
    std::fs::rename(&partial, &path)?;
    Ok(())
}

/// A version the French text cut: its row, its article and the first day it no longer applies.
pub(super) struct Version {
    pub id: i64,
    pub num: String,
    pub date_fin: Option<NaiveDate>,
}

/// A language's state of an act: the day it applies from, its file's checksum, its articles.
pub(super) struct State {
    pub day: NaiveDate,
    pub checksum: u64,
    pub articles: Vec<ConsolidatedArticle>,
}

/// Each version's article, matched on its key, in the state in force on the version's last day.
pub(super) fn hang<'a>(
    versions: &[Version],
    states: &'a [State],
) -> Vec<(i64, &'a ConsolidatedArticle, u64)> {
    let keyed: Vec<HashMap<String, &ConsolidatedArticle>> = states
        .iter()
        .map(|s| {
            s.articles
                .iter()
                .map(|a| (lj_norm::article_key::identity_key(&a.num), a))
                .collect()
        })
        .collect();
    versions
        .iter()
        .filter_map(|v| {
            let index = match v.date_fin {
                Some(fin) => states.partition_point(|s| s.day < fin).checked_sub(1)?,
                None => states.len().checked_sub(1)?,
            };
            let article = keyed[index].get(&lj_norm::article_key::identity_key(&v.num))?;
            Some((v.id, *article, states[index].checksum))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: &str) -> NaiveDate {
        NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()
    }

    fn state(d: &str, checksum: u64, arts: &[(&str, &str)]) -> State {
        State {
            day: day(d),
            checksum,
            articles: arts
                .iter()
                .map(|(num, texte)| ConsolidatedArticle {
                    num: num.to_string(),
                    title_path: None,
                    texte: texte.to_string(),
                    nota: None,
                })
                .collect(),
        }
    }

    fn version(id: i64, num: &str, fin: Option<&str>) -> Version {
        Version {
            id,
            num: num.to_string(),
            date_fin: fin.map(day),
        }
    }

    #[test]
    fn a_version_takes_the_state_in_force_on_its_last_day() {
        let states = [
            state("2010-01-01", 1, &[("1", "Erste"), ("2", "Zweite")]),
            state("2015-07-01", 2, &[("1", "Erste, neu"), ("2", "Zweite")]),
            state("2020-01-01", 3, &[("1", "Erste, neu"), ("3a", "Dritte a")]),
        ];
        let versions = [
            version(10, "1", Some("2015-07-01")),
            version(11, "1", None),
            version(20, "2", Some("2020-01-01")),
            version(30, "3a", None),
            version(40, "4", None),
            version(50, "1", Some("2010-01-01")),
        ];
        let hung: Vec<(i64, &str, u64)> = hang(&versions, &states)
            .into_iter()
            .map(|(id, a, checksum)| (id, a.texte.as_str(), checksum))
            .collect();
        assert_eq!(
            hung,
            [
                (10, "Erste", 1),
                (11, "Erste, neu", 3),
                (20, "Zweite", 2),
                (30, "Dritte a", 3)
            ]
        );
    }
}
