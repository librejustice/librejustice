//! Djibouti's texts as the eJO publishes them: one version each from its issue's day, its issue
//! and issuer, and the act its title amends.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{anyhow, Result};
use lj_parse::{EjoIssue, EjoNature, EjoText, SiteBody, SiteEntry};
use lj_pg::db::Connection;
use lj_sources::ejo::EjoClient;
use lj_store::repository::{
    DecisionRepository, Edition, IssuePlacement, JournalIssueRow, PublicationRow,
};
use serde_json::json;

use super::super::batch::drain_batch;
use super::super::court_sites::candidate;
use super::super::legal_corpus::{load_doc, CorpusArticle, CorpusDoc, CorpusLink};
use super::super::IngestMode;
use super::{reason, Run, Tally};

pub(super) const SOURCE: &str = "journalofficiel-dj";
/// `false` keeps the laws, ordinances, decrees and deliberations, and the arrêtés and décisions
/// whose title lays down general rules, none naming a person.
const EVERY_NATURE: bool = true;
const BATCH: usize = 100;
const JOURNAL: &str = "JO-DJ";
const EDITION: Edition = Edition {
    language: "fr",
    status: "authentic",
};

fn kept(nature: EjoNature, title: &str) -> bool {
    EVERY_NATURE
        || !lj_parse::ejo_individual(title)
            && (nature.normative()
                || matches!(nature, EjoNature::Arrete | EjoNature::Decision)
                    && lj_parse::ejo_general(title))
}

/// A text's uid from its nature and reference, by its post id when it has no reference to key.
fn uid(nature: EjoNature, reference: Option<&str>, post: u64) -> String {
    let key = reference
        .filter(|_| nature != EjoNature::Other)
        .map(lj_parse::ejo_reference_key)
        .filter(|k| !k.is_empty());
    match key {
        Some(key) => format!("DJ/{}-{key}", nature.code()),
        None => format!("DJ/{}-WP{post}", nature.code()),
    }
}

fn reference(t: &EjoText) -> Option<String> {
    t.reference
        .clone()
        .or_else(|| lj_parse::ejo_title_reference(&t.title))
}

fn title_key(title: &str) -> String {
    lj_fold::fold(title)
        .trim_end_matches(['.', ' '])
        .to_string()
}

/// Each post's uid, `None` for a text posted again: of a uid's posts, a text keeps its latest one,
/// and a different text under the same reference takes the uid suffixed with its post id.
fn identities(listed: &[&EjoText]) -> HashMap<u64, Option<String>> {
    let mut groups: BTreeMap<String, Vec<&EjoText>> = BTreeMap::new();
    for t in listed {
        let nature = EjoNature::of(&t.natures, &t.title);
        groups
            .entry(uid(nature, reference(t).as_deref(), t.id))
            .or_default()
            .push(t);
    }
    let mut out = HashMap::new();
    for (base, mut posts) in groups {
        posts.sort_by_key(|t| t.id);
        let mut texts: Vec<(String, Vec<u64>)> = Vec::new();
        for t in posts {
            let key = title_key(&t.title);
            match texts.iter_mut().find(|(k, _)| *k == key) {
                Some((_, ids)) => ids.push(t.id),
                None => texts.push((key, vec![t.id])),
            }
        }
        for (i, (_, ids)) in texts.into_iter().enumerate() {
            let latest = *ids.last().expect("a post");
            for id in ids {
                let uid = (id == latest).then(|| match i {
                    0 => base.clone(),
                    _ => format!("{base}-WP{latest}"),
                });
                out.insert(id, uid);
            }
        }
    }
    out
}

fn checksum(t: &EjoText) -> u64 {
    xxhash_rust::xxh3::xxh3_64(
        format!(
            "{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
            t.title,
            t.reference,
            t.natures,
            t.institutions,
            t.issue,
            t.content,
            t.visas,
            t.signature
        )
        .as_bytes(),
    )
}

fn day(d: Option<&str>) -> Option<chrono::NaiveDate> {
    d.and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

fn issue_title(issue: &EjoIssue) -> String {
    let number = issue.number.as_deref().unwrap_or("?");
    match day(issue.day.as_deref()) {
        Some(d) => format!(
            "Journal officiel de la République de Djibouti n° {number} du {}",
            d.format("%d/%m/%Y")
        ),
        None => format!("Journal officiel de la République de Djibouti n° {number}"),
    }
}

/// What one post writes: its text, or the Conseil constitutionnel's ruling it is, its
/// publication, and its place in its issue.
struct Read {
    doc: CorpusDoc,
    ruling: Option<SiteEntry>,
    publication: PublicationRow,
    facts: serde_json::Value,
    issue: Option<(JournalIssueRow, IssuePlacement)>,
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

/// A post as written, `None` when its content is empty. A text that numbers no article is one
/// piece keyed `body`.
fn read(
    t: &EjoText,
    uid: &str,
    issues: &HashMap<u64, EjoIssue>,
    institutions: &HashMap<u64, String>,
    rank: Option<i32>,
) -> Option<Read> {
    let content = t.content.as_deref().unwrap_or_default();
    let mut articles: Vec<CorpusArticle> = lj_parse::journal_articles(content)
        .into_iter()
        .map(|a| article(a.num, None, &a.texte, a.title_path))
        .collect();
    if articles.is_empty() {
        let body = lj_parse::journal_body(content);
        if body.is_empty() {
            return None;
        }
        articles.push(article(
            String::new(),
            Some("body".to_string()),
            &body,
            None,
        ));
    }
    let nature = EjoNature::of(&t.natures, &t.title);
    let issue = t.issue.and_then(|id| issues.get(&id));
    let published = issue.and_then(|i| i.day.clone());
    let title = t.title.trim_end_matches(['.', ' ']).to_string();
    let links = lj_parse::ejo_effects(&t.title)
        .into_iter()
        .map(|e| CorpusLink {
            verb: e.verb.to_string(),
            direction: "outgoing".to_string(),
            target_text_uid: uid_of(e.nature, &e.reference),
            target_label: e.label,
            date: None,
        })
        .filter(|l| l.target_text_uid != uid)
        .collect();
    let issuer: Vec<&str> = t
        .institutions
        .iter()
        .filter_map(|i| institutions.get(i).map(String::as_str))
        .collect();
    let doc = CorpusDoc {
        text_uid: uid.to_string(),
        source: SOURCE.to_string(),
        jurisdiction: "DJ".to_string(),
        title: title.clone(),
        title_key: None,
        nature: nature.nature().to_string(),
        translation: "officiel".to_string(),
        source_url: t.link.clone(),
        source_asof: None,
        source_upstream_url: None,
        date_texte: t.day.clone(),
        effect_date: published.clone().or_else(|| t.day.clone()),
        detect_abrogation: false,
        num_prefix_agnostic: false,
        articles,
        links,
    };
    let publication = PublicationRow {
        text_uid: uid.to_string(),
        journal_code: JOURNAL.to_string(),
        nature: Some(nature.nature().to_string()),
        nor: None,
        title: title.clone(),
        date_texte: day(t.day.as_deref()),
        date_publi: day(published.as_deref()),
        url: t.link.clone().unwrap_or_default(),
        body: None,
    };
    let mut blocks = serde_json::Map::new();
    if let Some(v) = &t.visas {
        blocks.insert("visas".to_string(), json!(v));
    }
    if let Some(s) = &t.signature {
        blocks.insert("signataires".to_string(), json!(s));
    }
    let facts = json!({
        "text_uid": uid,
        "date_publi": published,
        "num_parution": issue.and_then(|i| i.number.clone()),
        "autorite": (!issuer.is_empty()).then(|| issuer.join(", ")),
        "title": title,
        "keywords": [],
        "blocks": blocks,
    });
    let issue = issue.zip(rank).map(|(i, rank)| {
        let issue_uid = format!("{JOURNAL}/{}", i.id);
        (
            JournalIssueRow {
                issue_uid: issue_uid.clone(),
                journal_code: JOURNAL.to_string(),
                num: i.number.clone(),
                date_publi: day(i.day.as_deref()),
                eli: i.link.clone(),
                title: issue_title(i),
                toc: json!([]),
            },
            IssuePlacement {
                text_uid: uid.to_string(),
                issue_uid,
                issue_rank: rank,
            },
        )
    });
    Some(Read {
        doc,
        ruling: lj_parse::ejo_council_ruling(t).then(|| lj_parse::ejo_ruling(t)),
        publication,
        facts,
        issue,
    })
}

fn uid_of(nature: EjoNature, reference: &str) -> String {
    uid(nature, Some(reference), 0)
}

impl Run<'_> {
    /// Lists the eJO, then reads by hundreds the texts never visited and those changed since, or
    /// the `posts` asked alone, whatever their visit.
    pub(super) async fn ejo_lane(
        &self,
        client: &EjoClient,
        posts: &[u64],
        limit: Option<usize>,
    ) -> Tally {
        let mut tally = Tally::default();
        if let Err(e) = self.ejo(client, posts, limit, &mut tally).await {
            tracing::error!(publisher = self.source, error = %e, "eJO run stopped");
            tally.failed.push("ejo".to_string());
        }
        tally
    }

    async fn ejo(
        &self,
        client: &EjoClient,
        posts: &[u64],
        limit: Option<usize>,
        tally: &mut Tally,
    ) -> Result<()> {
        let latest = self.visits.values().map(|v| v.visited_at).max();
        let since = match (self.relist_before, latest) {
            (None, Some(latest)) if posts.is_empty() => Some(
                (latest - chrono::Duration::days(1))
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string(),
            ),
            _ => None,
        };
        let listed = client.listing(since.as_deref()).await?;
        let kept: Vec<&EjoText> = listed
            .iter()
            .filter(|t| kept(EjoNature::of(&t.natures, &t.title), &t.title))
            .collect();
        let identities = identities(&kept);
        let mut reposted: Vec<(String, u64)> = Vec::new();
        let mut to_read: Vec<(u64, String)> = Vec::new();
        for t in &kept {
            let item = t.id.to_string();
            let skipped = if posts.is_empty() {
                reason(day(t.modified.as_deref()), self.visits.get(&item), None).is_none()
            } else {
                !posts.contains(&t.id)
            };
            if skipped {
                tally.unchanged += 1;
                continue;
            }
            match &identities[&t.id] {
                None => reposted.push((item, checksum(t))),
                Some(uid) => to_read.push((t.id, uid.clone())),
            }
        }
        if let Some(limit) = limit {
            to_read.truncate(limit);
        }
        let read_ids: HashSet<u64> = to_read.iter().map(|(id, _)| *id).collect();
        let mut by_issue: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for t in kept.iter().filter(|t| read_ids.contains(&t.id)) {
            if let Some(issue) = t.issue {
                by_issue.entry(issue).or_default().push(t.id);
            }
        }
        let mut ranks: HashMap<u64, i32> = HashMap::new();
        for ids in by_issue.values_mut() {
            ids.sort_unstable();
            for (i, id) in ids.iter().enumerate() {
                ranks.insert(*id, i32::try_from(i + 1)?);
            }
        }
        let issue_ids: Vec<u64> = by_issue.keys().copied().collect();
        let mut issues: HashMap<u64, EjoIssue> = HashMap::new();
        for chunk in issue_ids.chunks(BATCH) {
            issues.extend(client.issues(chunk).await?.into_iter().map(|i| (i.id, i)));
        }
        tracing::info!(
            publisher = self.source,
            listed = listed.len(),
            kept = kept.len(),
            to_read = to_read.len(),
            reposted = reposted.len(),
            issues = issues.len(),
            since = since.as_deref(),
            "eJO texts listed"
        );
        let institutions = if to_read.is_empty() {
            HashMap::new()
        } else {
            client.institutions().await?
        };
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        if !self.dry_run && !reposted.is_empty() {
            repo.put_norm_visits(self.source, &reposted)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        let mut empty = 0usize;
        for chunk in to_read.chunks(BATCH) {
            let ids: Vec<u64> = chunk.iter().map(|(id, _)| *id).collect();
            let posts: HashMap<u64, EjoText> = client
                .contents(&ids)
                .await?
                .into_iter()
                .map(|t| (t.id, t))
                .collect();
            let mut visited: Vec<(String, u64)> = Vec::new();
            for (id, uid) in chunk {
                tally.listed += 1;
                let Some(t) = posts.get(id) else {
                    tracing::warn!(publisher = self.source, post = id, "a listed text is gone");
                    continue;
                };
                let Some(r) = read(t, uid, &issues, &institutions, ranks.get(id).copied()) else {
                    empty += 1;
                    visited.push((id.to_string(), checksum(t)));
                    continue;
                };
                if self.dry_run {
                    continue;
                }
                match self.ejo_write(&conn, &repo, &r, tally).await {
                    Ok(()) => visited.push((id.to_string(), checksum(t))),
                    Err(e) => {
                        tracing::error!(publisher = self.source, key = uid, post = id, error = %e, "followed text failed");
                        tally.failed.push(uid.clone());
                    }
                }
            }
            if !self.dry_run {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
            }
        }
        if empty > 0 {
            tracing::warn!(
                publisher = self.source,
                empty,
                "texts without any content are visited and not written"
            );
        }
        Ok(())
    }

    async fn ejo_write(
        &self,
        conn: &Connection,
        repo: &DecisionRepository<'_>,
        r: &Read,
        tally: &mut Tally,
    ) -> Result<()> {
        match &r.ruling {
            Some(entry) => self.ejo_ruling(conn, repo, entry, &r.doc.text_uid).await?,
            None => {
                load_doc(
                    repo,
                    &r.doc,
                    self.read_on,
                    self.source,
                    &mut tally.publie_targets,
                )
                .await?
            }
        }
        repo.upsert_publications(EDITION, std::slice::from_ref(&r.publication))
            .await
            .map_err(|e| anyhow!("upsert_publications {}: {e}", r.doc.text_uid))?;
        repo.upsert_publication_facts(EDITION, std::slice::from_ref(&r.facts))
            .await
            .map_err(|e| anyhow!("upsert_publication_facts {}: {e}", r.doc.text_uid))?;
        if let Some((issue, placement)) = &r.issue {
            repo.upsert_journal_issues(EDITION, std::slice::from_ref(issue))
                .await
                .map_err(|e| anyhow!("upsert_journal_issues {}: {e}", issue.issue_uid))?;
            repo.place_publications(EDITION, std::slice::from_ref(placement))
                .await
                .map_err(|e| anyhow!("place_publications {}: {e}", r.doc.text_uid))?;
        }
        tally.written += 1;
        Ok(())
    }

    /// A ruling of the Conseil constitutionnel is a decision of its court, never a norm: the norm
    /// an earlier read wrote under its uid goes. A ruling the parse refuses keeps its publication.
    async fn ejo_ruling(
        &self,
        conn: &Connection,
        repo: &DecisionRepository<'_>,
        entry: &SiteEntry,
        uid: &str,
    ) -> Result<()> {
        let SiteBody::Html { html, .. } = &entry.body else {
            unreachable!("an eJO ruling carries its text")
        };
        match candidate(SOURCE, entry, &lj_parse::html_text(html), false) {
            Ok(c) => {
                drain_batch(
                    conn,
                    vec![c],
                    IngestMode::MissingHash,
                    &mut Default::default(),
                )
                .await?;
            }
            Err(e) => {
                tracing::warn!(publisher = self.source, post = %entry.id, error = %e, "a Council ruling is refused, its publication alone written");
                return Ok(());
            }
        }
        if let Some(counts) = repo
            .delete_legal_text(uid)
            .await
            .map_err(|e| anyhow!("delete_legal_text {uid}: {e}"))?
        {
            tracing::info!(
                publisher = self.source,
                text_uid = uid,
                articles = counts.articles,
                "a Council ruling's norm is dropped for its decision"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(id: u64, title: &str, reference: &str) -> EjoText {
        EjoText {
            id,
            day: None,
            modified: None,
            title: title.to_string(),
            reference: Some(reference.to_string()),
            natures: vec![],
            institutions: vec![],
            issue: None,
            link: None,
            content: None,
            visas: None,
            signature: None,
        }
    }

    #[test]
    fn a_text_posted_twice_keeps_its_latest_post_and_another_text_takes_its_post_id() {
        let posts = [
            post(
                204610,
                "Loi n° 16/AN/23/9ème L Relative aux aires protégées.",
                "16/AN/23/9ème L",
            ),
            post(
                209752,
                "Loi n° 16/AN/23/9ème L relative aux aires protégées",
                "16/AN/23/9ème L",
            ),
            post(
                114112,
                "DELIBERATION n° 489/6e L rendant exécutoire la délibération",
                "489/6e L",
            ),
            post(
                114575,
                "DELIBERATION n° 489/6e L portant institution d'un régime",
                "489/6e L",
            ),
            post(5, "Rapport sur l'exercice", "5"),
        ];
        let ids = identities(&posts.iter().collect::<Vec<_>>());
        assert_eq!(ids[&204610], None);
        assert_eq!(ids[&209752].as_deref(), Some("DJ/LOI-16-AN-23-9L"));
        assert_eq!(ids[&114112].as_deref(), Some("DJ/DELIBERATION-489-6L"));
        assert_eq!(
            ids[&114575].as_deref(),
            Some("DJ/DELIBERATION-489-6L-WP114575")
        );
        assert_eq!(ids[&5].as_deref(), Some("DJ/TEXTE-WP5"));
    }
}
