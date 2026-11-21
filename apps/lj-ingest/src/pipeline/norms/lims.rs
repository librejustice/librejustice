//! Québec's and New Brunswick's codified texts, from the LIMS pages a browser fetched into the cache.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use chrono::{NaiveDate, Utc};
use lj_norm::text_aliases::TextAlias;
use lj_parse::{ConsolidatedArticle, LimsDocument};
use lj_store::repository::{ArticleLanguageWrite, DecisionRepository};

use super::super::legal_corpus::{
    article_status, load_doc, run_corpus_finishers, CorpusArticle, CorpusDoc, CorpusVersion,
};
use super::expressions;
use crate::config::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Act,
    Regulation,
}

impl Kind {
    /// The folder the fetch keeps a kind's pages in, and its listing's name.
    fn folder(self) -> &'static str {
        match self {
            Self::Act => "lc",
            Self::Regulation => "rc",
        }
    }
}

pub(super) struct Site {
    pub(super) source: &'static str,
    /// The uid's head, `CA-QC` in `CA-QC/A-2.1`.
    province: &'static str,
    place: &'static str,
    /// The cache folder the fetch fills.
    folder: &'static str,
    page: fn(Kind, &str) -> String,
    /// The citation in French and in English.
    citation: fn(Kind, &str) -> (String, Option<String>),
}

pub(super) const LEGISQUEBEC: Site = Site {
    source: "legisquebec",
    province: "CA-QC",
    place: "Québec",
    folder: "legisquebec",
    page: |kind, id| {
        format!(
            "https://www.legisquebec.gouv.qc.ca/fr/document/{}/{}",
            kind.folder(),
            id.replace(' ', "%20")
        )
    },
    citation: |_, id| (format!("RLRQ, c. {id}"), Some(format!("CQLR, c. {id}"))),
};

pub(super) const NEW_BRUNSWICK: Site = Site {
    source: "gnb",
    province: "CA-NB",
    place: "Nouveau-Brunswick",
    folder: "gnb",
    page: |kind, id| {
        format!(
            "https://lois.gnb.ca/fr/document/{}/{}",
            kind.folder(),
            id.replace(' ', "%20")
        )
    },
    citation: |kind, id| match kind {
        Kind::Act => match id.split_once(", c.") {
            Some((year, chapter)) => (format!("{year}, ch. {chapter}"), None),
            None => (format!("ch. {id}"), None),
        },
        Kind::Regulation => (format!("Règl. du N.-B. {id}"), Some(format!("NB Reg {id}"))),
    },
};

/// `A-2.1, r. 2` keys as `A-2.1-r2`, `A-3.001, r. 11 (2011)` as `A-3.001-r11-2011`.
fn key(id: &str) -> Result<String> {
    let key = id
        .replace(", r. ", "-r")
        .replace(", c.", "-c")
        .replace(" (", "-")
        .replace(')', "")
        .replace(", ", "-")
        .replace(' ', "-");
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        bail!("{id:?} keys as {key:?}");
    }
    Ok(key)
}

/// The file stem the fetch gave a page: every run outside `[0-9A-Za-z.,_-]` one `_`.
fn stem(id: &str) -> String {
    let mut out = String::new();
    for c in id.trim_matches([' ', '/']).chars() {
        if c.is_ascii_alphanumeric() || ".,_-".contains(c) {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out
}

fn read_page(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(String::from_utf8(zstd::decode_all(&bytes[..])?)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("{}: {e}", path.display())),
    }
}

/// A listing's ids, `%20` read as a space.
fn listing(pages: &Path, kind: Kind) -> Result<Vec<String>> {
    let path = pages.join(format!("{}.txt", kind.folder()));
    let raw = std::fs::read_to_string(&path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    Ok(raw
        .lines()
        .map(|l| l.replace("%20", " ").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

struct Text {
    kind: Kind,
    id: String,
    key: String,
    french: String,
    english: Option<String>,
}

impl Text {
    fn checksum(&self) -> u64 {
        let english = self.english.as_deref().unwrap_or_default();
        xxhash_rust::xxh3::xxh3_64(&[self.french.as_bytes(), english.as_bytes()].concat())
    }
}

fn clean(articles: Vec<(String, ConsolidatedArticle)>) -> Vec<(String, ConsolidatedArticle)> {
    articles
        .into_iter()
        .map(|(code, mut a)| {
            a.texte = lj_core::normalizer::normalize_chars(&a.texte);
            a.nota = a.nota.as_deref().map(lj_core::normalizer::normalize_chars);
            (code, a)
        })
        .collect()
}

impl Site {
    /// `Loi sur les abeilles (Québec), RLRQ, c. A-1`, `Général - Loi sur… (Nouveau-Brunswick), Règl. du N.-B. 84-20`.
    fn title(&self, kind: Kind, id: &str, doc: &LimsDocument) -> String {
        let name = match (&doc.enabling, kind, self.province) {
            (Some(act), Kind::Regulation, "CA-NB") => format!("{} - {act}", doc.title),
            _ => doc.title.clone(),
        };
        let place = match name.contains(self.place) {
            true => String::new(),
            false => format!(" ({})", self.place),
        };
        format!("{name}{place}, {}", (self.citation)(kind, id).0)
    }

    fn aliases(
        &self,
        text: &Text,
        french: &LimsDocument,
        english: Option<&LimsDocument>,
    ) -> Vec<TextAlias> {
        let (fr, en) = (self.citation)(text.kind, &text.id);
        let mut aliases = vec![TextAlias::number(&fr)];
        aliases.extend(en.as_deref().map(TextAlias::number));
        if text.kind == Kind::Act {
            aliases.push(TextAlias::title("fr", &french.title));
            aliases.extend(english.map(|e| TextAlias::title("en", &e.title)));
        }
        aliases
    }

    fn doc(&self, text: &Text, uid: &str, french: &LimsDocument, read_on: NaiveDate) -> CorpusDoc {
        let ended = french.ended.as_deref().filter(|_| french.repealed);
        let articles = french
            .articles
            .iter()
            .map(|(_, a)| CorpusArticle {
                num: a.num.clone(),
                num_key: None,
                texte: ended.is_none().then(|| a.texte.clone()),
                texte_original: None,
                lang_original: None,
                translation: None,
                nota: a.nota.clone(),
                versions: ended.map(|day| {
                    vec![CorpusVersion {
                        date_debut: None,
                        date_fin: Some(day.to_string()),
                        status: article_status(&a.texte).to_string(),
                        texte: a.texte.clone(),
                        source: None,
                        source_url: None,
                        source_asof: None,
                        source_upstream_url: None,
                    }]
                }),
                title_path: a.title_path.clone(),
            })
            .collect();
        CorpusDoc {
            text_uid: uid.to_string(),
            source: self.source.to_string(),
            jurisdiction: "CA".to_string(),
            title: self.title(text.kind, &text.id, french),
            title_key: None,
            nature: match text.kind {
                Kind::Act if french.title.starts_with("Code ") => "code",
                Kind::Act => "loi",
                Kind::Regulation => "reglement",
            }
            .to_string(),
            translation: "officiel".to_string(),
            source_url: Some((self.page)(text.kind, &text.id)),
            source_asof: french.asof.clone().or_else(|| Some(read_on.to_string())),
            source_upstream_url: None,
            date_texte: match (self.province, text.kind) {
                ("CA-QC", Kind::Regulation) => None,
                _ => french.enacted.clone(),
            },
            effect_date: None,
            detect_abrogation: true,
            num_prefix_agnostic: false,
            articles,
            links: Vec::new(),
        }
    }
}

/// The English articles under the French numbers their codes carry, with how many had none.
fn english_under_french(
    french: &LimsDocument,
    english: Vec<(String, ConsolidatedArticle)>,
) -> (Vec<ConsolidatedArticle>, usize) {
    let nums: HashMap<&str, &str> = french
        .articles
        .iter()
        .map(|(code, a)| (code.as_str(), a.num.as_str()))
        .collect();
    let mut orphans = 0usize;
    let articles = english
        .into_iter()
        .filter_map(|(code, mut a)| match nums.get(code.as_str()) {
            Some(num) => {
                a.num = num.to_string();
                Some(a)
            }
            None => {
                orphans += 1;
                None
            }
        })
        .collect();
    (articles, orphans)
}

#[derive(Default, Debug)]
struct Counts {
    listed: usize,
    no_page: usize,
    unchanged: usize,
    failed: usize,
    written: usize,
    repealed: usize,
    english: usize,
    articles: usize,
    english_orphans: usize,
}

pub(super) async fn sync(
    site: &Site,
    only: Option<&str>,
    limit: Option<usize>,
    fresh: bool,
    dry_run: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pages: PathBuf = settings.cache_dir().join(site.folder).join("pages");
    let read_on = Utc::now().date_naive();
    let pool = match dry_run {
        true => None,
        false => Some(
            lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?,
        ),
    };
    let visits = match &pool {
        Some(pool) => {
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            lj_store::migrator::apply_migrations(
                &conn,
                lj_store::migrator::MigrationIntent::Verify,
            )
            .await
            .map_err(|e| anyhow!("migrations: {e}"))?;
            match fresh {
                true => Default::default(),
                false => DecisionRepository::new(&conn)
                    .norm_visits(site.source)
                    .await
                    .map_err(|e| anyhow!("norm_visits {}: {e}", site.source))?,
            }
        }
        None => Default::default(),
    };
    let mut counts = Counts::default();
    let mut publie_targets = HashSet::new();
    for kind in [Kind::Act, Kind::Regulation] {
        for id in listing(&pages, kind)? {
            if limit.is_some_and(|l| counts.written >= l) {
                break;
            }
            let key = key(&id)?;
            if only.is_some_and(|o| o != key) {
                continue;
            }
            counts.listed += 1;
            let dir = pages.join(kind.folder());
            let page = |language: &str| dir.join(format!("{}.{language}.html.zst", stem(&id)));
            let Some(french) = read_page(&page("fr"))? else {
                counts.no_page += 1;
                continue;
            };
            let text = Text {
                kind,
                english: read_page(&page("en"))?,
                id,
                key,
                french,
            };
            let checksum = text.checksum();
            if visits
                .get(&text.key)
                .is_some_and(|v| v.content_checksum == checksum)
            {
                counts.unchanged += 1;
                continue;
            }
            let read = lj_parse::lims_document(&text.french).and_then(|fr| {
                let en = text
                    .english
                    .as_deref()
                    .map(lj_parse::lims_document)
                    .transpose()?;
                Ok((fr, en))
            });
            let (mut french, english) = match read {
                Ok(read) => read,
                Err(e) => {
                    tracing::error!(
                        publisher = site.source,
                        key = text.key.as_str(),
                        error = %e,
                        "lims page unread"
                    );
                    counts.failed += 1;
                    continue;
                }
            };
            if french.articles.is_empty() || (french.repealed && french.ended.is_none()) {
                tracing::error!(
                    publisher = site.source,
                    key = text.key.as_str(),
                    articles = french.articles.len(),
                    repealed = french.repealed,
                    "lims text without an article, or repealed without its day"
                );
                counts.failed += 1;
                continue;
            }
            french.articles = clean(std::mem::take(&mut french.articles));
            let uid = format!("{}/{}", site.province, text.key);
            let doc = site.doc(&text, &uid, &french, read_on);
            let aliases = site.aliases(&text, &french, english.as_ref());
            let hung = english.map(|en| english_under_french(&french, clean(en.articles)));
            counts.articles += french.articles.len();
            counts.repealed += usize::from(french.repealed);
            counts.english += usize::from(hung.is_some());
            counts.english_orphans += hung.as_ref().map_or(0, |(_, orphans)| *orphans);
            tracing::info!(
                publisher = site.source,
                text_uid = uid.as_str(),
                title = doc.title.as_str(),
                articles = french.articles.len(),
                repealed = french.repealed,
                english = hung.as_ref().map(|(a, _)| a.len()),
                "lims text read"
            );
            let Some(pool) = &pool else {
                counts.written += 1;
                continue;
            };
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            let repo = DecisionRepository::new(&conn);
            load_doc(&repo, &doc, read_on, site.source, &mut publie_targets).await?;
            let rows: Vec<(&str, &TextAlias)> = aliases.iter().map(|a| (uid.as_str(), a)).collect();
            repo.put_text_aliases(&[&uid], &rows)
                .await
                .map_err(|e| anyhow!("put_text_aliases {uid}: {e}"))?;
            if let Some((articles, _)) = hung {
                hang_english(&repo, &uid, articles, checksum).await?;
            }
            repo.put_norm_visits(site.source, &[(text.key.clone(), checksum)])
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", site.source))?;
            counts.written += 1;
        }
    }
    tracing::info!(publisher = site.source, dry_run, counts = ?counts, "lims cache loaded");
    if let (Some(pool), true) = (&pool, counts.written > 0) {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        run_corpus_finishers(&DecisionRepository::new(&conn), publie_targets).await?;
    }
    Ok(())
}

/// The English of each version the French text cut, one state standing for the whole text.
async fn hang_english(
    repo: &DecisionRepository<'_>,
    uid: &str,
    articles: Vec<ConsolidatedArticle>,
    checksum: u64,
) -> Result<()> {
    let versions: Vec<expressions::Version> = repo
        .article_version_ends(uid)
        .await
        .map_err(|e| anyhow!("article_version_ends {uid}: {e}"))?
        .into_iter()
        .map(|(id, num, date_fin)| expressions::Version { id, num, date_fin })
        .collect();
    let state = expressions::State {
        day: NaiveDate::MIN,
        checksum,
        articles,
    };
    let rows: Vec<ArticleLanguageWrite> =
        expressions::hang(&versions, std::slice::from_ref(&state))
            .into_iter()
            .map(|(article_id, a, checksum)| ArticleLanguageWrite {
                article_id,
                text_status: "official".to_string(),
                texte: a.texte.clone(),
                title_path: a.title_path.clone(),
                content_checksum: i64::from_ne_bytes(checksum.to_ne_bytes()),
            })
            .collect();
    repo.replace_language_texts(uid, "en", &rows)
        .await
        .map_err(|e| anyhow!("replace_language_texts {uid} en: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_keys_without_its_spaces_and_finds_the_stem_the_fetch_gave_its_page() {
        assert_eq!(key("A-2.1").unwrap(), "A-2.1");
        assert_eq!(key("A-2.1, r. 2").unwrap(), "A-2.1-r2");
        assert_eq!(key("A-2.1, r. 0.1").unwrap(), "A-2.1-r0.1");
        assert_eq!(key("A-3.001, r. 11 (2011)").unwrap(), "A-3.001-r11-2011");
        assert_eq!(key("2011, c.23").unwrap(), "2011-c23");
        assert_eq!(key("2016, c.28, s.1").unwrap(), "2016-c28-s.1");
        assert_eq!(key("84-20").unwrap(), "84-20");
        assert!(key("A/1").is_err());
        assert_eq!(stem("A-2.1, r. 2"), "A-2.1,_r._2");
        assert_eq!(stem("2011, c.23"), "2011,_c.23");
        assert_eq!(stem("A-3.001, r. 11 (2011)"), "A-3.001,_r._11_2011_");
    }
}
