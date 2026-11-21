//! Ontario's e-Laws: each dated version a state, French where it has a French page, English hung.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use lj_norm::text_aliases::TextAlias;
use lj_parse::{ConsolidatedArticle, ElawsKind, ElawsListed};
use lj_sources::elaws::ElawsClient;
use lj_store::repository::{ArticleLanguageWrite, DecisionRepository};

use super::super::legal_corpus::{load_doc, CorpusDoc};
use super::{aliases, article_versions, expressions, refused, Run, Tally, Target};

pub(super) const SOURCE: &str = "elaws";

/// A version: its number, `None` when current, its days, its page's change, its French page.
pub(super) struct Version {
    number: Option<u32>,
    from: NaiveDate,
    until: Option<NaiveDate>,
    lastmod: String,
    french: bool,
}

pub(super) struct Text {
    kind: ElawsKind,
    /// Oldest first, one a day.
    versions: Vec<Version>,
}

impl Text {
    /// Read in French when its last version has a French page.
    fn french(&self) -> bool {
        self.versions.last().is_some_and(|v| v.french)
    }
}

pub(super) type Index = HashMap<String, Text>;

/// When a version's kept page stands for it.
enum Kept {
    /// Its visit holds its entry's token.
    Always,
    /// Never visited: when kept after the day its page last changed.
    After(NaiveDate),
    Never,
}

fn checksum(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

fn day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// `S.O. 1992, c. 6` in `Class Proceedings Act, 1992, S.O. 1992, c. 6`.
fn statute_names(title: &str) -> Vec<TextAlias> {
    match title
        .rsplit_once(", S.O. ")
        .or_else(|| title.rsplit_once(", R.S.O. "))
    {
        Some((name, _)) => vec![
            TextAlias::title("en", name.trim()),
            TextAlias::number(title[name.len() + 2..].trim()),
        ],
        None => vec![TextAlias::title("en", title.trim())],
    }
}

/// Every text the English sitemaps date, keyed by its e-Laws key, `92c06` or `900194`.
pub(super) fn targets(
    english: Vec<ElawsListed>,
    french: Vec<ElawsListed>,
    selected: &dyn Fn(&str) -> bool,
) -> (Vec<Target>, Index) {
    let in_french: HashSet<(ElawsKind, String, Option<u32>)> = french
        .into_iter()
        .map(|l| (l.kind, l.key, l.version))
        .collect();
    let mut grouped: HashMap<String, (ElawsKind, String, Vec<Version>)> = HashMap::new();
    let mut undated = 0usize;
    for l in english {
        let Some(from) = l.from.as_deref().and_then(day) else {
            undated += 1;
            continue;
        };
        let entry = grouped
            .entry(l.key.clone())
            .or_insert_with(|| (l.kind, String::new(), Vec::new()));
        if l.version.is_none() || entry.1.is_empty() {
            entry.1 = l.title.clone();
        }
        entry.2.push(Version {
            french: in_french.contains(&(l.kind, l.key.clone(), l.version)),
            number: l.version,
            from,
            until: l.to.as_deref().and_then(day),
            lastmod: l.lastmod,
        });
    }
    let mut keys: Vec<String> = grouped.keys().filter(|k| selected(k)).cloned().collect();
    keys.sort();
    let (mut targets, mut index) = (Vec::new(), Index::new());
    for key in keys {
        let (kind, title, mut versions) = grouped.remove(&key).expect("a key grouped");
        versions.sort_by_key(|v| (v.from, v.number.is_none(), v.number));
        let mut by_day: Vec<Version> = Vec::with_capacity(versions.len());
        for v in versions {
            match by_day.last_mut() {
                Some(last) if last.from == v.from => *last = v,
                _ => by_day.push(v),
            }
        }
        let tokens: String = by_day
            .iter()
            .map(|v| {
                format!(
                    "{:?}|{}|{:?}|{}|{};",
                    v.number, v.from, v.until, v.lastmod, v.french
                )
            })
            .collect();
        let text = Text {
            kind,
            versions: by_day,
        };
        let last = text.versions.last().expect("a text has a version");
        targets.push(Target {
            url: ElawsClient::page_url(kind, &key, last.number, text.french()),
            text_uid: format!("CA-ON/{key}"),
            title: format!("{} (Ontario)", title.trim()),
            title_key: None,
            nature: match kind {
                ElawsKind::Statute => "loi",
                ElawsKind::Regulation => "reglement",
            }
            .to_string(),
            current: Some(checksum(&tokens)),
            justel: None,
            aliases: match kind {
                ElawsKind::Statute => statute_names(&title),
                ElawsKind::Regulation => Vec::new(),
            },
            key: key.clone(),
        });
        index.insert(key, text);
    }
    tracing::info!(
        texts = targets.len(),
        french = index.values().filter(|t| t.french()).count(),
        versions = index.values().map(|t| t.versions.len()).sum::<usize>(),
        undated,
        "e-laws sitemaps read"
    );
    (targets, index)
}

impl Run<'_> {
    /// The texts whose versions moved since their visit, or whose English was never hung.
    pub(super) async fn elaws_lane(
        &self,
        client: &ElawsClient,
        index: &Index,
        targets: &[Target],
        limit: Option<usize>,
    ) -> Tally {
        let mut tally = Tally::default();
        let mut left = 0usize;
        for t in targets {
            if limit.is_some_and(|l| tally.listed >= l) {
                break;
            }
            let text = &index[&t.key];
            let hung = !text.french()
                || self
                    .visits
                    .contains_key(&expressions::text_item(&t.key, "en"));
            if hung && self.unchanged(t, None) {
                tally.unchanged += 1;
                continue;
            }
            if self.left_for_later(&t.key) {
                left += 1;
                continue;
            }
            tally.listed += 1;
            if let Err(e) = self.elaws_text(client, text, t, &mut tally).await {
                tracing::error!(publisher = SOURCE, key = t.key, error = %e, "ontario text failed");
                tally.failed.push(t.key.clone());
                if refused(&e) {
                    break;
                }
            }
        }
        self.budget_spent(SOURCE, left);
        tally
    }

    /// A version's page from the run's files when `kept` allows it, else from the site, then kept.
    async fn elaws_page(
        &self,
        client: &ElawsClient,
        key: &str,
        from: NaiveDate,
        language: &str,
        url: &str,
        kept: Kept,
    ) -> Result<String> {
        let stands = match kept {
            Kept::Always => true,
            Kept::After(changed) => expressions::kept_on(&self.cache, SOURCE, key, from, language)?
                .is_some_and(|on| on > changed),
            Kept::Never => false,
        };
        if stands {
            if let Some(html) = expressions::cached(&self.cache, SOURCE, key, from, language)? {
                return Ok(html);
            }
        }
        let html = client.html(url).await?;
        expressions::cache(&self.cache, SOURCE, key, from, language, &html)?;
        Ok(html)
    }

    /// Each version in a language as a state, the French ones alone when `french_versions`.
    async fn elaws_states(
        &self,
        client: &ElawsClient,
        text: &Text,
        t: &Target,
        (french, french_versions): (bool, bool),
        item: &dyn Fn(NaiveDate) -> String,
    ) -> Result<(Vec<expressions::State>, Vec<(String, u64)>, Option<String>)> {
        let language = if french { "fr" } else { "en" };
        let (mut states, mut read, mut title) = (Vec::new(), Vec::new(), None);
        for v in text
            .versions
            .iter()
            .filter(|v| v.french || !french_versions)
        {
            let url = ElawsClient::page_url(text.kind, &t.key, v.number, french);
            let token = checksum(&format!("{url}|{}|{:?}", v.lastmod, v.until));
            let item = item(v.from);
            let kept = match self.visits.get(&item) {
                Some(visit) if visit.content_checksum == token => Kept::Always,
                Some(_) => Kept::Never,
                None => day(&v.lastmod).map_or(Kept::Never, Kept::After),
            };
            let fresh = !matches!(kept, Kept::Always);
            let html = self
                .elaws_page(client, &t.key, v.from, language, &url, kept)
                .await?;
            if fresh {
                read.push((item, token));
            }
            title = lj_parse::elaws_title(&html).or(title);
            let mut articles: Vec<ConsolidatedArticle> =
                lj_parse::elaws_articles(&html).map_err(|e| anyhow!("{url}: {e}"))?;
            for a in &mut articles {
                a.texte = lj_core::normalizer::normalize_chars(&a.texte);
                a.nota = a.nota.as_deref().map(lj_core::normalizer::normalize_chars);
            }
            if articles.is_empty() {
                tracing::warn!(
                    publisher = SOURCE,
                    url,
                    "a version without any article is left out"
                );
                continue;
            }
            states.push(expressions::State {
                day: v.from,
                checksum: token,
                articles,
            });
        }
        Ok((states, read, title))
    }

    /// A text's versions, closed after the last day of a text no longer in force, English hung.
    async fn elaws_text(
        &self,
        client: &ElawsClient,
        text: &Text,
        t: &Target,
        tally: &mut Tally,
    ) -> Result<()> {
        let french = text.french();
        let item = |from: NaiveDate| format!("{}@{from}", t.key);
        let (states, mut read, title) = self
            .elaws_states(client, text, t, (french, french), &item)
            .await?;
        let rewrite = !read.is_empty() || !self.visits.contains_key(&t.key);
        tracing::info!(
            publisher = SOURCE,
            key = t.key,
            french,
            states = states.len(),
            read = read.len(),
            "ontario text listed"
        );
        if states.is_empty() {
            bail!("{} holds no article in any version", t.key);
        }
        if rewrite {
            let mut kept: Vec<(NaiveDate, Vec<ConsolidatedArticle>)> =
                states.into_iter().map(|s| (s.day, s.articles)).collect();
            let ended = text.versions.last().and_then(|v| v.until);
            if let Some(next) = ended
                .filter(|d| *d < self.read_on)
                .and_then(|d| d.succ_opt())
            {
                kept.push((next, Vec::new()));
            }
            let articles = article_versions(&kept);
            let versions: usize = articles
                .iter()
                .map(|a| a.versions.as_ref().map_or(0, Vec::len))
                .sum();
            tracing::info!(
                publisher = SOURCE,
                key = t.key,
                text_uid = t.text_uid,
                articles = articles.len(),
                versions,
                ended = ?ended,
                "ontario text rebuilt"
            );
            if !self.dry_run {
                let conn = self
                    .pool
                    .get()
                    .await
                    .map_err(|e| anyhow!("pool.get: {e}"))?;
                let repo = DecisionRepository::new(&conn);
                let doc = CorpusDoc {
                    text_uid: t.text_uid.clone(),
                    source: SOURCE.to_string(),
                    jurisdiction: self.jurisdiction.to_string(),
                    title: title.unwrap_or_else(|| t.title.clone()),
                    title_key: None,
                    nature: t.nature.clone(),
                    translation: "officiel".to_string(),
                    source_url: Some(t.url.clone()),
                    source_asof: None,
                    source_upstream_url: None,
                    date_texte: None,
                    effect_date: None,
                    detect_abrogation: false,
                    num_prefix_agnostic: false,
                    articles,
                    links: Vec::new(),
                };
                load_doc(&repo, &doc, self.read_on, SOURCE, &mut tally.publie_targets).await?;
                tally.written += 1;
                let names = aliases(t);
                let rows: Vec<(&str, &TextAlias)> =
                    names.iter().map(|a| (t.text_uid.as_str(), a)).collect();
                repo.put_text_aliases(&[&t.text_uid], &rows)
                    .await
                    .map_err(|e| anyhow!("put_text_aliases {}: {e}", t.text_uid))?;
            }
        }
        if !self.dry_run {
            read.push((t.key.clone(), t.current.unwrap_or_default()));
            let conn = self
                .pool
                .get()
                .await
                .map_err(|e| anyhow!("pool.get: {e}"))?;
            DecisionRepository::new(&conn)
                .put_norm_visits(SOURCE, &read)
                .await
                .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))?;
        }
        if french {
            self.elaws_english(client, text, t, rewrite).await?;
        }
        Ok(())
    }

    /// Hangs the English pages of a French text's versions on the versions its French cut.
    async fn elaws_english(
        &self,
        client: &ElawsClient,
        text: &Text,
        t: &Target,
        rehang: bool,
    ) -> Result<()> {
        const LANGUAGE: &str = "en";
        let item = |from: NaiveDate| expressions::state_item(&t.key, from, LANGUAGE);
        let (states, mut read, _) = self
            .elaws_states(client, text, t, (false, true), &item)
            .await?;
        let days: String = states.iter().map(|s| s.day.to_string()).collect();
        let days = checksum(&days);
        let text_item = expressions::text_item(&t.key, LANGUAGE);
        let hung_before = self
            .visits
            .get(&text_item)
            .is_some_and(|v| v.content_checksum == days);
        tracing::info!(
            publisher = SOURCE,
            key = t.key,
            language = LANGUAGE,
            states = states.len(),
            read = read.len(),
            "ontario text's English states read"
        );
        if self.dry_run || (read.is_empty() && !rehang && hung_before) {
            return Ok(());
        }
        read.push((text_item, days));
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let versions: Vec<expressions::Version> = repo
            .article_version_ends(&t.text_uid)
            .await
            .map_err(|e| anyhow!("article_version_ends {}: {e}", t.text_uid))?
            .into_iter()
            .map(|(id, num, date_fin)| expressions::Version { id, num, date_fin })
            .collect();
        let rows: Vec<ArticleLanguageWrite> = expressions::hang(&versions, &states)
            .into_iter()
            .map(|(article_id, a, checksum)| ArticleLanguageWrite {
                article_id,
                text_status: "official".to_string(),
                texte: a.texte.clone(),
                title_path: a.title_path.clone(),
                content_checksum: i64::from_ne_bytes(checksum.to_ne_bytes()),
            })
            .collect();
        let written = repo
            .replace_language_texts(&t.text_uid, LANGUAGE, &rows)
            .await
            .map_err(|e| anyhow!("replace_language_texts {} {LANGUAGE}: {e}", t.text_uid))?;
        tracing::info!(
            publisher = SOURCE,
            key = t.key,
            text_uid = t.text_uid,
            versions = versions.len(),
            hung = rows.len(),
            written,
            "ontario text's versions hung in English"
        );
        repo.put_norm_visits(SOURCE, &read)
            .await
            .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))
    }
}
