//! The issues of a JORF archive and the publication of each of its texts,
//! merged member by member into `journal_issue` and `text_publication`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_store::repository::{DecisionRepository, Edition, IssuePlacement, JournalIssueRow};
use serde_json::{json, Map, Value};

use super::jorf::{classify_jorf_member, JorfMember};
use crate::config::Settings;

const BATCH: usize = 1000;
const JORF: Edition = Edition {
    language: "fr",
    status: "authentic",
};

enum Msg {
    Issue(JournalIssueRow, Vec<String>),
    Facts(String, Value),
    Structure(String, Value),
    Section(String, String, Value),
    Article(String, String, Value),
    Deleted(Vec<String>),
    Failed,
}

#[derive(Default)]
struct Pending {
    issues: Vec<JournalIssueRow>,
    placements: HashMap<String, (String, i32)>,
    facts: HashMap<String, Value>,
    structures: HashMap<String, Value>,
    sections: HashMap<String, Map<String, Value>>,
    articles: Vec<(String, String, Value)>,
    entries: usize,
}

#[derive(Default, Debug)]
struct Counts {
    issues: u64,
    placed: u64,
    facts: u64,
    structures: u64,
    sections: u64,
    articles: u64,
    deleted: u64,
    failed: usize,
}

fn date(iso: Option<&str>) -> Result<Option<NaiveDate>> {
    iso.map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|e| anyhow!("date {d:?}: {e}")))
        .transpose()
}

fn member_msg(name: &str, raw: &[u8]) -> Result<Option<Msg>> {
    Ok(Some(match classify_jorf_member(name) {
        JorfMember::Issue => {
            let issue = lj_extract::jorf::parse_jorf_conteneur(raw)?;
            let texts = issue.texts().into_iter().map(str::to_string).collect();
            let row = JournalIssueRow {
                date_publi: date(issue.date_publi.as_deref())?,
                toc: serde_json::to_value(&issue.toc)?,
                issue_uid: issue.issue_uid,
                journal_code: "JORF".to_string(),
                num: issue.num,
                eli: issue.eli,
                title: issue.title,
            };
            Msg::Issue(row, texts)
        }
        JorfMember::Texte => {
            let p = lj_extract::jorf::parse_jorf_publication(raw)?;
            date(p.date_publi.as_deref())?;
            let facts = json!({
                "text_uid": p.jorftext,
                "nature": p.nature,
                "num_parution": p.num_parution,
                "num_sequence": p.num_sequence,
                "origine_publi": p.origine_publi,
                "date_publi": p.date_publi,
                "eli": p.eli,
                "page_deb": p.page_deb,
                "page_fin": p.page_fin,
                "ministere": p.ministere,
                "autorite": p.autorite,
                "title": p.title,
                "title_full": p.title_full,
                "keywords": p.keywords,
                "enterprise": p.enterprise,
                "blocks": p.blocks,
            });
            Msg::Facts(p.jorftext, facts)
        }
        JorfMember::Struct => {
            let s = lj_extract::jorf::parse_jorf_struct(raw)?;
            Msg::Structure(s.jorftext, serde_json::to_value(&s.items)?)
        }
        JorfMember::Section => {
            let s = lj_extract::jorf::parse_jorf_section(raw)?;
            Msg::Section(s.jorftext, s.id, serde_json::to_value(&s.body)?)
        }
        JorfMember::Article => {
            let a = lj_extract::jorf::parse_jorf_article(raw)?;
            let Some(text) = a.published_text() else {
                return Ok(None);
            };
            Msg::Article(a.jorftext, a.jorfarti, serde_json::to_value(&text)?)
        }
        JorfMember::Ignore => return Ok(None),
    }))
}

impl Pending {
    fn push(&mut self, msg: Msg, deleted: &mut Vec<String>, counts: &mut Counts) {
        match msg {
            Msg::Issue(row, texts) => {
                for (rank, text) in texts.into_iter().enumerate() {
                    self.placements
                        .insert(text, (row.issue_uid.clone(), rank as i32 + 1));
                }
                self.issues.push(row);
            }
            Msg::Facts(text, facts) => {
                self.facts.insert(text, facts);
            }
            Msg::Structure(text, structure) => {
                self.structures.insert(text, structure);
            }
            Msg::Section(text, id, body) => {
                self.sections.entry(text).or_default().insert(id, body);
                self.entries += 1;
            }
            Msg::Article(text, id, body) => {
                self.articles.push((text, id, body));
                self.entries += 1;
            }
            Msg::Deleted(paths) => deleted.extend(paths),
            Msg::Failed => counts.failed += 1,
        }
    }

    fn full(&self) -> bool {
        self.issues.len() >= BATCH / 10
            || self.facts.len() >= BATCH
            || self.structures.len() >= BATCH
            || self.entries >= BATCH
    }

    async fn flush(
        &mut self,
        repo: &DecisionRepository<'_>,
        counts: &mut Counts,
        with_entries: &mut HashSet<String>,
    ) -> Result<()> {
        let p = std::mem::take(self);
        if !p.issues.is_empty() {
            counts.issues += repo.upsert_journal_issues(JORF, &p.issues).await?;
            let placements: Vec<IssuePlacement> = p
                .placements
                .into_iter()
                .map(|(text_uid, (issue_uid, issue_rank))| IssuePlacement {
                    text_uid,
                    issue_uid,
                    issue_rank,
                })
                .collect();
            counts.placed += repo.place_publications(JORF, &placements).await?;
        }
        if !p.facts.is_empty() {
            let facts: Vec<Value> = p.facts.into_values().collect();
            counts.facts += repo.upsert_publication_facts(JORF, &facts).await?;
        }
        if !p.structures.is_empty() {
            let rows: Vec<(String, Value)> = p.structures.into_iter().collect();
            counts.structures += repo.upsert_publication_structures(JORF, &rows).await?;
        }
        if !p.sections.is_empty() {
            counts.sections += repo
                .merge_publication_sections(JORF, &maps(p.sections))
                .await?;
        }
        if !p.articles.is_empty() {
            let mut articles: HashMap<String, Map<String, Value>> = HashMap::new();
            for (text, id, body) in p.articles {
                articles.entry(text).or_default().insert(id, body);
            }
            with_entries.extend(articles.keys().cloned());
            counts.articles += repo
                .merge_publication_articles(JORF, &maps(articles))
                .await?;
        }
        Ok(())
    }
}

fn maps(by_text: HashMap<String, Map<String, Value>>) -> Vec<(String, Value)> {
    by_text
        .into_iter()
        .map(|(text, entries)| (text, Value::Object(entries)))
        .collect()
}

/// The members a deletion list names: issues, texts, sections, articles.
fn deleted_members(paths: &[String]) -> [Vec<String>; 4] {
    let mut kinds: [Vec<String>; 4] = Default::default();
    for path in paths {
        let id = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".xml");
        let slot = match id.get(..8) {
            Some("JORFCONT") => 0,
            Some("JORFTEXT") => 1,
            Some("JORFSCTA") => 2,
            Some("JORFARTI") => 3,
            _ => continue,
        };
        if !kinds[slot].iter().any(|k| k == id) {
            kinds[slot].push(id.to_string());
        }
    }
    kinds
}

/// Reads one archive for its issues and publications, then applies its
/// deletion list. Returns the texts whose articles took entries.
pub(super) async fn ingest_publications(
    repo: &DecisionRepository<'_>,
    path: &Path,
) -> Result<HashSet<String>> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Msg>(BATCH * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            let msg = if name.to_lowercase().ends_with(".dat") {
                let paths = String::from_utf8_lossy(&raw)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                Msg::Deleted(paths)
            } else {
                match member_msg(&name, &raw) {
                    Ok(Some(msg)) => msg,
                    Ok(None) => return Ok(()),
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "jorf publication: member failed");
                        Msg::Failed
                    }
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("jorf publication channel closed"))
        })
    });

    let mut pending = Pending::default();
    let mut deleted = Vec::new();
    let mut counts = Counts::default();
    let mut with_entries = HashSet::new();
    while let Some(msg) = rx.recv().await {
        pending.push(msg, &mut deleted, &mut counts);
        if pending.full() {
            pending.flush(repo, &mut counts, &mut with_entries).await?;
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("jorf publication reader {}: {e}", path.display()))??;
    pending.flush(repo, &mut counts, &mut with_entries).await?;
    let [issues, texts, sections, articles] = deleted_members(&deleted);
    counts.deleted = repo
        .delete_publication_members(&issues, &texts, &sections, &articles)
        .await?;
    tracing::info!(source = %path.display(), ?counts, "jorf publications");
    Ok(with_entries)
}

/// Replays every cached JORF archive, stock then increments, then moves the
/// unnumbered articles into pieces.
pub async fn backfill_jorf_publications() -> Result<()> {
    let settings = Settings::from_env()?;
    let paths = lj_sources::dila::cached_dila_tarballs(
        &settings.cache_dir(),
        lj_sources::dila::DilaFond::Jorf,
    )?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let started = std::time::Instant::now();
    for path in &paths {
        ingest_publications(&repo, path).await?;
    }
    let pieces = super::jorf_pieces::move_pieces(&repo, None).await?;
    tracing::warn!(
        archives = paths.len(),
        ?pieces,
        seconds = started.elapsed().as_secs(),
        "jorf publications: replayed"
    );
    Ok(())
}
