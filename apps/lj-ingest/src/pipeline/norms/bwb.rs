//! The Netherlands' regelingen from the BWB: each consolidated state its manifest names is a
//! state of the text, the regelingen in force read before those that ended.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_norm::text_aliases::TextAlias;
use lj_parse::{BwbListed, BwbState, ConsolidatedArticle};
use lj_sources::bwb::BwbClient;
use lj_store::repository::{DecisionRepository, StoredArticleVersion};

use super::super::legal_corpus::{load_doc, CorpusArticle, CorpusDoc};
use super::{aliases, article_versions, refused, stored_state, Run, Tally, Target};

pub(super) const SOURCE: &str = "bwb";
/// A run reads at most this much of one regeling's XML, newest state first: the Algemene wet
/// bestuursrecht's 187 states weigh about 0.8 GB.
const BUDGET_BYTES: usize = 1 << 30;

fn checksum(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

/// The visit of a regeling whose manifest showed it ended before any of its states was read.
fn ended_item(id: &str) -> String {
    format!("{id}#ended")
}

/// The visit of a regeling's abbreviations, read once from its WTI.
fn names_item(id: &str) -> String {
    format!("{id}#wti")
}

fn ended(states: &[BwbState], today: NaiveDate) -> bool {
    states
        .last()
        .and_then(|s| s.until.as_deref())
        .is_some_and(|until| until < today.to_string().as_str())
}

/// The regelingen the sitemap lists, the latest changed first.
pub(super) fn targets(mut listed: Vec<BwbListed>, selected: &dyn Fn(&str) -> bool) -> Vec<Target> {
    listed.retain(|l| l.id.starts_with("BWBR") && selected(&l.id));
    listed.sort_by(|a, b| b.lastmod.cmp(&a.lastmod).then_with(|| a.id.cmp(&b.id)));
    listed
        .into_iter()
        .map(|l| Target {
            url: BwbClient::page_url(&l.id),
            text_uid: format!("NL/{}", l.id),
            title: String::new(),
            title_key: None,
            nature: String::new(),
            current: Some(checksum(&l.lastmod)),
            justel: None,
            aliases: Vec::new(),
            key: l.id,
        })
        .collect()
}

fn nature(soort: &str, title: &str) -> &'static str {
    let soort = soort.to_lowercase();
    if title == "Grondwet" || title.starts_with("Statuut voor het Koninkrijk") {
        "CONSTITUTION"
    } else if title.starts_with("Burgerlijk Wetboek") || title.starts_with("Wetboek ") {
        "CODE"
    } else if soort.contains("verdrag") {
        "TRAITE"
    } else if soort.contains("amvb") || soort.starts_with("kb") {
        "DECRET"
    } else if soort.starts_with("wet") || soort.starts_with("rijkswet") {
        "LOI"
    } else if soort.starts_with("ministeriele-regeling") {
        "ARRETE"
    } else if soort.starts_with("zbo") || soort.starts_with("pbo") || soort.contains("reglement") {
        "REGLEMENT"
    } else if soort.contains("beleidsregel") || soort.contains("circulaire") {
        "CIRCULAIRE"
    } else {
        "ACTE"
    }
}

/// An article's first version opens on the day its wording came into force rather than on the
/// oldest state's, as the state that read it or the base says.
fn open_first_versions(
    articles: &mut [CorpusArticle],
    oldest: NaiveDate,
    since: Option<&HashMap<String, String>>,
    stored: &[StoredArticleVersion],
) {
    let mut earliest: HashMap<&str, NaiveDate> = HashMap::new();
    for v in stored {
        let day = earliest.entry(v.num.as_str()).or_insert(v.date_debut);
        *day = (*day).min(v.date_debut);
    }
    let oldest_day = oldest.to_string();
    for a in articles {
        let Some(first) = a.versions.as_mut().and_then(|v| v.first_mut()) else {
            continue;
        };
        if first.date_debut.as_deref() != Some(oldest_day.as_str()) {
            continue;
        }
        let read = since
            .and_then(|s| s.get(&a.num))
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let start = [read, earliest.get(a.num.as_str()).copied()]
            .into_iter()
            .flatten()
            .filter(|d| *d < oldest)
            .min();
        if let Some(day) = start {
            first.date_debut = Some(day.to_string());
        }
    }
}

impl Run<'_> {
    /// The regelingen whose sitemap day moved since their visit, in force first: one that ended
    /// and was never read waits for the end of the run, marked so the next run needs no manifest.
    pub(super) async fn bwb_lane(
        &self,
        client: &BwbClient,
        targets: &[Target],
        limit: Option<usize>,
    ) -> Tally {
        let mut tally = Tally::default();
        let mut later: Vec<(&Target, Option<Vec<BwbState>>)> = Vec::new();
        let mut left = 0usize;
        for t in targets {
            if limit.is_some_and(|l| tally.listed >= l) {
                break;
            }
            if self
                .visits
                .get(&t.key)
                .is_some_and(|v| Some(v.content_checksum) == t.current)
            {
                tally.unchanged += 1;
                if !self.bwb_names_once(client, t, &mut tally).await {
                    return tally;
                }
                continue;
            }
            let never_read = !self.visits.contains_key(&t.key);
            if never_read
                && self
                    .visits
                    .get(&ended_item(&t.key))
                    .is_some_and(|v| Some(v.content_checksum) == t.current)
            {
                later.push((t, None));
                continue;
            }
            if self.left_for_later(&t.key) {
                left += 1;
                continue;
            }
            tally.listed += 1;
            let states = match client.states(&t.key).await {
                Ok(states) => states,
                Err(e) => {
                    let e = anyhow!(e);
                    tracing::error!(publisher = SOURCE, key = t.key, error = %e, "manifest failed");
                    tally.failed.push(t.key.clone());
                    if refused(&e) {
                        return tally;
                    }
                    continue;
                }
            };
            if never_read && ended(&states, self.read_on) {
                if let Err(e) = self.mark_ended(t).await {
                    tracing::error!(publisher = SOURCE, key = t.key, error = %e, "ended mark not written");
                }
                later.push((t, Some(states)));
                continue;
            }
            match self.bwb_text(client, t, states, &mut tally).await {
                Ok(()) => {
                    if !self.bwb_names_once(client, t, &mut tally).await {
                        return tally;
                    }
                }
                Err(e) => {
                    tracing::error!(publisher = SOURCE, key = t.key, error = %e, "regeling failed");
                    tally.failed.push(t.key.clone());
                    if refused(&e) {
                        return tally;
                    }
                }
            }
        }
        tracing::info!(
            publisher = SOURCE,
            ended = later.len(),
            "regelingen in force read, those that ended follow"
        );
        for (t, states) in later {
            if limit.is_some_and(|l| tally.listed >= l) {
                break;
            }
            if self.left_for_later(&t.key) {
                left += 1;
                continue;
            }
            let read = async {
                let states = match states {
                    Some(states) => states,
                    None => {
                        tally.listed += 1;
                        client.states(&t.key).await?
                    }
                };
                self.bwb_text(client, t, states, &mut tally).await
            };
            match read.await {
                Ok(()) => {
                    if !self.bwb_names_once(client, t, &mut tally).await {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(publisher = SOURCE, key = t.key, error = %e, "regeling failed");
                    tally.failed.push(t.key.clone());
                    if refused(&e) {
                        break;
                    }
                }
            }
        }
        self.budget_spent(SOURCE, left);
        tally
    }

    /// Reads the regeling's abbreviations unless a visit did; false once the publisher refuses.
    async fn bwb_names_once(&self, client: &BwbClient, t: &Target, tally: &mut Tally) -> bool {
        if self.visits.contains_key(&names_item(&t.key)) {
            return true;
        }
        match self.bwb_names(client, t).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(publisher = SOURCE, key = t.key, error = %e, "abbreviations failed");
                tally.failed.push(names_item(&t.key));
                !refused(&e)
            }
        }
    }

    async fn bwb_names(&self, client: &BwbClient, t: &Target) -> Result<()> {
        let names = client.abbreviations(&t.key).await?;
        if self.dry_run {
            return Ok(());
        }
        let named: Vec<TextAlias> = names
            .iter()
            .map(|n| TextAlias::abbreviation("nl", n))
            .collect();
        let all = [aliases(t), named].concat();
        let rows: Vec<(&str, &TextAlias)> = all.iter().map(|a| (t.text_uid.as_str(), a)).collect();
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        repo.put_text_aliases(&[&t.text_uid], &rows)
            .await
            .map_err(|e| anyhow!("put_text_aliases {}: {e}", t.text_uid))?;
        repo.put_norm_visits(SOURCE, &[(names_item(&t.key), checksum(&names.join("\n")))])
            .await
            .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))
    }

    async fn mark_ended(&self, t: &Target) -> Result<()> {
        let Some(current) = t.current.filter(|_| !self.dry_run) else {
            return Ok(());
        };
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        DecisionRepository::new(&conn)
            .put_norm_visits(SOURCE, &[(ended_item(&t.key), current)])
            .await
            .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))
    }

    /// The states no visit read, newest first within [`BUDGET_BYTES`], the newest whenever any
    /// is, which names the text; a regeling that ended closes its articles after its last day.
    async fn bwb_text(
        &self,
        client: &BwbClient,
        t: &Target,
        states: Vec<BwbState>,
        tally: &mut Tally,
    ) -> Result<()> {
        let tokens: Vec<(String, u64)> = states
            .iter()
            .map(|s| {
                let token = format!("{}{:?}", s.hash, s.until);
                (format!("{}@{}", t.key, s.applies_from), checksum(&token))
            })
            .collect();
        let due: Vec<bool> = tokens
            .iter()
            .map(|(item, token)| {
                self.visits
                    .get(item)
                    .is_none_or(|v| v.content_checksum != *token)
            })
            .collect();
        let any_due = due.iter().any(|d| *d);
        let mut kept: Vec<(NaiveDate, Option<Vec<ConsolidatedArticle>>)> = Vec::new();
        let mut read: Vec<(String, u64)> = Vec::new();
        let mut newest = None;
        let mut since: Option<(NaiveDate, HashMap<String, String>)> = None;
        let (mut bytes, mut cut, mut repeated) = (0usize, None, 0usize);
        for (i, s) in states.iter().enumerate().rev() {
            let day = NaiveDate::parse_from_str(&s.applies_from, "%Y-%m-%d")
                .map_err(|e| anyhow!("{} state {:?}: {e}", t.key, s.applies_from))?;
            let wanted = due[i] || (any_due && i + 1 == states.len());
            if !wanted {
                kept.push((day, None));
                continue;
            }
            if bytes >= BUDGET_BYTES {
                cut = Some(s.applies_from.clone());
                break;
            }
            let (toestand, size) = client.state(&t.key, s).await?;
            bytes += size;
            repeated += toestand.repeated.len();
            let mut articles = toestand.articles;
            for a in &mut articles {
                a.texte = lj_core::normalizer::normalize_chars(&a.texte);
            }
            read.push(tokens[i].clone());
            if newest.is_none() {
                newest = Some((toestand.title, toestand.soort, toestand.adopted));
            }
            since = Some((day, toestand.since));
            if articles.is_empty() {
                continue;
            }
            kept.push((day, Some(articles)));
        }
        kept.reverse();
        if let Some(day) = &cut {
            tracing::warn!(
                publisher = SOURCE,
                key = t.key,
                bytes,
                history_from = ?kept.first().map(|k| k.0),
                cut_at = day,
                "regeling past its download budget, its history starts at the earliest state read"
            );
        }
        if let Some(until) = states.last().and_then(|s| s.until.as_deref()) {
            let last = NaiveDate::parse_from_str(until, "%Y-%m-%d")
                .map_err(|e| anyhow!("{} end {until:?}: {e}", t.key))?;
            if let Some(next) = last.succ_opt() {
                kept.push((next, Some(Vec::new())));
            }
        }
        tracing::info!(
            publisher = SOURCE,
            key = t.key,
            states = states.len(),
            read = read.len(),
            repeated,
            mb = bytes / 1_000_000,
            "regeling listed"
        );
        let mut visits = read;
        visits.push((t.key.clone(), t.current.unwrap_or_default()));
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let written = kept
            .iter()
            .any(|(_, a)| a.as_ref().is_some_and(|a| !a.is_empty()));
        if let (Some((title, soort, adopted)), true) = (newest, written) {
            let stored = if kept.iter().any(|(_, a)| a.is_none()) {
                repo.stored_article_versions(&t.text_uid)
                    .await
                    .map_err(|e| anyhow!("stored_article_versions {}: {e}", t.text_uid))?
            } else {
                Vec::new()
            };
            let kept: Vec<(NaiveDate, Vec<ConsolidatedArticle>)> = kept
                .into_iter()
                .map(|(day, arts)| (day, arts.unwrap_or_else(|| stored_state(&stored, day))))
                .collect();
            let mut articles = article_versions(&kept);
            let oldest = kept[0].0;
            let read_since = since
                .as_ref()
                .filter(|(day, _)| *day == oldest)
                .map(|(_, s)| s);
            open_first_versions(&mut articles, oldest, read_since, &stored);
            for a in &mut articles {
                if a.num.is_empty() {
                    a.num_key = Some("body".to_string());
                }
            }
            let versions: usize = articles
                .iter()
                .map(|a| a.versions.as_ref().map_or(0, Vec::len))
                .sum();
            tracing::info!(
                publisher = SOURCE,
                key = t.key,
                title,
                soort,
                articles = articles.len(),
                versions,
                "regeling rebuilt"
            );
            if self.dry_run {
                return Ok(());
            }
            let doc = CorpusDoc {
                text_uid: t.text_uid.clone(),
                source: SOURCE.to_string(),
                jurisdiction: self.jurisdiction.to_string(),
                nature: nature(&soort, &title).to_string(),
                title,
                title_key: None,
                translation: "officiel".to_string(),
                source_url: Some(t.url.clone()),
                source_asof: None,
                source_upstream_url: None,
                date_texte: adopted,
                effect_date: None,
                detect_abrogation: false,
                num_prefix_agnostic: false,
                articles,
                links: Vec::new(),
            };
            load_doc(&repo, &doc, self.read_on, SOURCE, &mut tally.publie_targets).await?;
            tally.written += 1;
        }
        if self.dry_run {
            return Ok(());
        }
        repo.put_norm_visits(SOURCE, &visits)
            .await
            .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(from: &str, until: Option<&str>) -> BwbState {
        BwbState {
            applies_from: from.to_string(),
            until: until.map(str::to_string),
            hash: String::new(),
            path: String::new(),
        }
    }

    #[test]
    fn a_regeling_ended_once_its_last_state_closed_before_today() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        assert!(ended(&[state("2002-01-01", Some("2026-11-18"))], today));
        assert!(!ended(&[state("2002-01-01", Some("2026-11-19"))], today));
        assert!(!ended(&[state("2026-07-01", None)], today));
        assert!(!ended(&[], today));
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_first_version_opens_on_the_day_its_wording_came_into_force() {
        let art = |num: &str, texte: &str| ConsolidatedArticle {
            num: num.to_string(),
            title_path: None,
            texte: texte.to_string(),
            nota: None,
        };
        let states = vec![
            (day("2002-04-12"), vec![art("1", "a"), art("2", "b")]),
            (
                day("2005-01-01"),
                vec![art("1", "a"), art("2", "b2"), art("3", "c")],
            ),
        ];
        let since = HashMap::from([
            ("1".to_string(), "1992-01-01".to_string()),
            ("2".to_string(), "2002-04-12".to_string()),
        ]);
        let mut articles = article_versions(&states);
        open_first_versions(&mut articles, day("2002-04-12"), Some(&since), &[]);
        let starts: Vec<(&str, Vec<Option<&str>>)> = articles
            .iter()
            .map(|a| {
                let v = a.versions.as_ref().unwrap();
                (
                    a.num.as_str(),
                    v.iter().map(|v| v.date_debut.as_deref()).collect(),
                )
            })
            .collect();
        assert_eq!(
            starts,
            [
                ("1", vec![Some("1992-01-01")]),
                ("2", vec![Some("2002-04-12"), Some("2005-01-01")]),
                ("3", vec![Some("2005-01-01")]),
            ]
        );
        let stored = [StoredArticleVersion {
            num: "1".to_string(),
            position: Some(0),
            title_path: None,
            nota: None,
            texte: "a".to_string(),
            date_debut: day("1992-01-01"),
            date_fin: None,
        }];
        let mut rebuilt = article_versions(&states);
        open_first_versions(&mut rebuilt, day("2002-04-12"), None, &stored);
        assert_eq!(
            rebuilt[0].versions.as_ref().unwrap()[0]
                .date_debut
                .as_deref(),
            Some("1992-01-01")
        );
    }

    #[test]
    fn a_regeling_s_nature_is_read_from_its_kind_and_title() {
        assert_eq!(nature("wet", "Grondwet"), "CONSTITUTION");
        assert_eq!(nature("wet", "Burgerlijk Wetboek Boek 6"), "CODE");
        assert_eq!(nature("wet", "Wetboek van Strafrecht"), "CODE");
        assert_eq!(nature("wet", "Algemene wet bestuursrecht"), "LOI");
        assert_eq!(nature("AMvB", "Bouwbesluit"), "DECRET");
        assert_eq!(nature("ministeriele-regeling", "Regeling x"), "ARRETE");
        assert_eq!(nature("pbo", "Verordening Bosschap"), "REGLEMENT");
        assert_eq!(nature("beleidsregel", "Terugwerkende kracht"), "CIRCULAIRE");
    }
}
