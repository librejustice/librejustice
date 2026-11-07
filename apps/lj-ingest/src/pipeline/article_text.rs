//! Rewrites what an unchanged DILA member now reads differently: article texte
//! and nota, JORF bodies built from articles, and the spans indexing into them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_core::normalizer::normalize_chars;
use lj_extract_rows::{build_declared, owner_rows, Declared, OwnerRows};
use lj_pg::db::Connection;
use lj_sources::dila::DilaFond;
use lj_store::repository::{
    ArticleTextDrift, ArticleTextRead, ArticleTextWrite, BodyTextWrite, DecisionRepository,
};
use rayon::prelude::*;

use super::jorf::{classify_jorf_member, JorfMember};
use super::kali::{classify_kali_member, KaliMember};
use super::legi::{classify_legi_member, LegiMember};
use super::ExtractCtx;
use crate::config::Settings;

/// Versions compared, and bodies read, per round trip.
const BATCH: usize = 1_000;
/// Members parsed per rayon round.
const CHUNK: usize = 4_096;

/// A DILA fond whose articles the referential stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ArticleFond {
    Jorf,
    Legi,
    Kali,
}

impl ArticleFond {
    fn dila(self) -> DilaFond {
        match self {
            Self::Jorf => DilaFond::Jorf,
            Self::Legi => DilaFond::Legi,
            Self::Kali => DilaFond::Kali,
        }
    }
}

/// Rereads every cached archive of `fond`, or of the three, and rewrites what
/// reads differently; `dry_run` counts it and writes nothing.
pub async fn backfill_article_text(fond: Option<ArticleFond>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let session = if dry_run {
        "SET statement_timeout = 0"
    } else {
        "SET statement_timeout = 0; \
         SET session_replication_role = replica; \
         SET synchronous_commit = off"
    };
    conn.batch_execute(session)
        .await
        .map_err(|e| anyhow!("session setup: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let spans = if dry_run {
        None
    } else {
        Some(Spans::load(&conn, &repo).await?)
    };
    let fonds = match fond {
        Some(f) => vec![f],
        None => vec![ArticleFond::Jorf, ArticleFond::Legi, ArticleFond::Kali],
    };
    for fond in fonds {
        backfill_fond(&repo, &settings, fond, spans).await?;
    }
    if spans.is_some() {
        let resolved = repo
            .resolve_pending_text_case_citations()
            .await
            .map_err(|e| anyhow!("resolve_pending_text_case_citations: {e}"))?;
        tracing::warn!(
            resolved,
            "backfill-article-text: pending case citations resolved"
        );
    }
    Ok(())
}

/// What the span extraction reads, loaded only for a run that writes.
#[derive(Clone, Copy)]
struct Spans {
    ctx: &'static ExtractCtx,
    declared: &'static Declared,
}

impl Spans {
    async fn load(conn: &Connection, repo: &DecisionRepository<'_>) -> Result<Self> {
        let ctx = super::extract_ctx(conn).await?;
        let declared = build_declared(repo.declared_article_cites().await?);
        Ok(Self {
            ctx,
            declared: Box::leak(Box::new(declared)),
        })
    }

    fn rows(
        self,
        texte: &str,
        text_uid: &str,
        num_key: Option<String>,
        date_debut: Option<NaiveDate>,
    ) -> OwnerRows {
        owner_rows(
            texte,
            text_uid,
            num_key,
            date_debut,
            &self.ctx.link,
            &self.ctx.vocab,
            &self.ctx.chrono,
            self.declared,
        )
    }
}

#[derive(Default)]
struct Tally {
    archives: usize,
    members: usize,
    unreadable: usize,
    /// Each drifted version once, though members repeat across archives.
    drifted: HashSet<(String, String, NaiveDate)>,
    versions_written: u64,
    bodies_compared: usize,
    bodies_current: usize,
    bodies_drifted: usize,
    bodies_written: u64,
    bodies_unmatched: usize,
}

async fn backfill_fond(
    repo: &DecisionRepository<'_>,
    settings: &Settings,
    fond: ArticleFond,
    spans: Option<Spans>,
) -> Result<()> {
    let paths = lj_sources::dila::cached_dila_tarballs(&settings.cache_dir(), fond.dila())?;
    let bodies: HashSet<String> = match fond {
        ArticleFond::Jorf => repo
            .legal_text_uids_with_body("JORFTEXT")
            .await
            .map_err(|e| anyhow!("legal_text_uids_with_body: {e}"))?
            .into_iter()
            .collect(),
        ArticleFond::Legi | ArticleFond::Kali => HashSet::new(),
    };
    let mut tally = Tally {
        archives: paths.len(),
        ..Tally::default()
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(bool, Vec<Member>)>(4);
    let bodies = Arc::new(bodies);
    let reader = {
        let bodies = bodies.clone();
        tokio::task::spawn_blocking(move || read_archives(fond, &paths, &bodies, &tx))
    };
    let mut reads: Vec<ArticleTextRead> = Vec::new();
    let mut parts: HashMap<String, BodyParts> = HashMap::new();
    while let Some((stock, members)) = rx.recv().await {
        for member in members {
            tally.members += 1;
            match member {
                Member::Article { read, block } => {
                    reads.extend(read);
                    if let Some(block) = block {
                        parts
                            .entry(block.text_uid.clone())
                            .or_default()
                            .push(block, stock);
                    }
                }
                Member::Order { text_uid, ids } => {
                    parts.entry(text_uid).or_default().order = Some(ids)
                }
                Member::Unreadable => tally.unreadable += 1,
            }
        }
        while reads.len() >= BATCH {
            let batch: Vec<ArticleTextRead> = reads.drain(..BATCH).collect();
            rewrite_articles(repo, batch, spans, &mut tally).await?;
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("backfill-article-text reader: {e}"))??;
    if !reads.is_empty() {
        rewrite_articles(repo, reads, spans, &mut tally).await?;
    }
    rewrite_bodies(repo, parts, spans, &mut tally).await?;
    tracing::warn!(
        fond = ?fond,
        dry_run = spans.is_none(),
        archives = tally.archives,
        members = tally.members,
        unreadable = tally.unreadable,
        versions_drifted = tally.drifted.len(),
        versions_written = tally.versions_written,
        bodies_compared = tally.bodies_compared,
        bodies_current = tally.bodies_current,
        bodies_drifted = tally.bodies_drifted,
        bodies_written = tally.bodies_written,
        bodies_unmatched = tally.bodies_unmatched,
        "backfill-article-text"
    );
    Ok(())
}

/// What a member gives the pass.
enum Member {
    Article {
        read: Option<ArticleTextRead>,
        block: Option<Block>,
    },
    Order {
        text_uid: String,
        ids: Vec<String>,
    },
    Unreadable,
}

/// The text of a JORF article whose text carries a body.
struct Block {
    text_uid: String,
    id: String,
    texte: String,
    numbered: bool,
}

/// Streams the archives in replay order, parsing each chunk of members in
/// parallel; the flag sent with a chunk says it comes from the stock.
fn read_archives(
    fond: ArticleFond,
    paths: &[PathBuf],
    bodies: &HashSet<String>,
    tx: &tokio::sync::mpsc::Sender<(bool, Vec<Member>)>,
) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        let stock = is_stock(path);
        let send = |chunk: Vec<(String, Vec<u8>)>| -> Result<()> {
            let members = chunk
                .into_par_iter()
                .map(|(name, raw)| read_member(fond, &name, &raw, bodies))
                .collect();
            tx.blocking_send((stock, members))
                .map_err(|_| anyhow!("backfill-article-text: consumer stopped"))
        };
        let mut chunk: Vec<(String, Vec<u8>)> = Vec::with_capacity(CHUNK);
        lj_sources::tar_reader::for_each_member_filtered(
            path,
            |name| wanted(fond, name, stock, bodies),
            |name, raw| {
                chunk.push((name, raw));
                if chunk.len() == CHUNK {
                    send(std::mem::take(&mut chunk))?;
                }
                Ok(())
            },
        )?;
        send(chunk)?;
        tracing::info!(archive = %path.display(), n = i + 1, of = paths.len(), "backfill-article-text: archive read");
    }
    Ok(())
}

fn is_stock(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("Freemium_"))
}

/// The members the pass reads: every article, and on JORF the stock structs of
/// the texts that carry a body.
fn wanted(fond: ArticleFond, name: &str, stock: bool, bodies: &HashSet<String>) -> bool {
    match fond {
        ArticleFond::Legi => matches!(classify_legi_member(name), LegiMember::Article),
        ArticleFond::Kali => matches!(classify_kali_member(name), KaliMember::Article),
        ArticleFond::Jorf => match classify_jorf_member(name) {
            JorfMember::Article => true,
            JorfMember::Struct => {
                let stem = name.rsplit('/').next().unwrap_or(name);
                stock && bodies.contains(stem.trim_end_matches(".xml"))
            }
            JorfMember::Texte | JorfMember::Section | JorfMember::Issue | JorfMember::Ignore => {
                false
            }
        },
    }
}

/// Reads a member as the ingest maps it: the versions it skips are skipped here.
fn read_member(fond: ArticleFond, name: &str, raw: &[u8], bodies: &HashSet<String>) -> Member {
    let checksum = xxhash_rust::xxh3::xxh3_64(raw);
    let member = match fond {
        ArticleFond::Legi => lj_extract::legi::parse_legi_article(raw).map(|a| Member::Article {
            read: (!a.num_key.is_empty()).then_some(ArticleTextRead {
                source_uid: a.legiarti,
                content_checksum: checksum,
                texte: a.texte,
                nota: a.nota,
            }),
            block: None,
        }),
        ArticleFond::Kali => lj_extract::kali::parse_kali_article(raw).map(|a| Member::Article {
            read: (!a.num_key.is_empty()).then_some(ArticleTextRead {
                source_uid: a.kaliarti,
                content_checksum: checksum,
                texte: a.texte,
                nota: None,
            }),
            block: None,
        }),
        ArticleFond::Jorf if matches!(classify_jorf_member(name), JorfMember::Struct) => {
            lj_extract::jorf::parse_jorf_struct(raw).map(|s| Member::Order {
                ids: s.articles().map(|(id, _)| id.to_string()).collect(),
                text_uid: s.jorftext,
            })
        }
        ArticleFond::Jorf => lj_extract::jorf::parse_jorf_article(raw).map(|a| {
            let numbered = a.num.is_some();
            let block = match &a.texte {
                Some(texte) if bodies.contains(&a.jorftext) => Some(Block {
                    text_uid: a.jorftext.clone(),
                    id: a.jorfarti.clone(),
                    texte: texte.clone(),
                    numbered,
                }),
                _ => None,
            };
            let read = (numbered && a.num_key.is_some()).then_some(ArticleTextRead {
                source_uid: a.jorfarti,
                content_checksum: checksum,
                texte: a.texte,
                nota: a.nota,
            });
            Member::Article { read, block }
        }),
    };
    member.unwrap_or_else(|e| {
        tracing::error!(member = %name, error = %e, "backfill-article-text: parse failed");
        Member::Unreadable
    })
}

/// `None` spans make a dry run: the drift is counted, nothing is written.
async fn rewrite_articles(
    repo: &DecisionRepository<'_>,
    reads: Vec<ArticleTextRead>,
    spans: Option<Spans>,
    tally: &mut Tally,
) -> Result<()> {
    let drifts: Vec<ArticleTextDrift> = repo
        .article_text_drift(&reads)
        .await
        .map_err(|e| anyhow!("article_text_drift: {e}"))?
        .into_iter()
        .filter(|d| {
            let key = (d.text_uid.clone(), d.num_key.clone(), d.date_debut);
            tally.drifted.insert(key)
        })
        .collect();
    let Some(spans) = spans else {
        return Ok(());
    };
    let writes: Vec<ArticleTextWrite> = tokio::task::spawn_blocking(move || {
        drifts
            .into_par_iter()
            .map(|drift| {
                let OwnerRows { cases, cites } = match &drift.texte {
                    Some(texte) => spans.rows(
                        texte,
                        &drift.text_uid,
                        Some(drift.num_key.clone()),
                        Some(drift.date_debut),
                    ),
                    None => OwnerRows {
                        cases: Vec::new(),
                        cites: Vec::new(),
                    },
                };
                ArticleTextWrite {
                    drift,
                    cases,
                    cites,
                }
            })
            .collect()
    })
    .await
    .map_err(|e| anyhow!("article spans join: {e}"))?;
    tally.versions_written += repo
        .rewrite_article_texts(&writes)
        .await
        .map_err(|e| anyhow!("rewrite_article_texts: {e}"))?;
    Ok(())
}

/// The articles of a JORF text that carries a body.
#[derive(Default)]
struct BodyParts {
    /// Unnumbered articles of every archive, a later version replacing an earlier.
    unnumbered: BTreeMap<String, String>,
    stock: Vec<(String, String)>,
    order: Option<Vec<String>>,
}

impl BodyParts {
    fn push(&mut self, block: Block, stock: bool) {
        if stock {
            self.stock.push((block.id.clone(), block.texte.clone()));
        }
        if !block.numbered {
            self.unnumbered.insert(block.id, block.texte);
        }
    }

    /// The bodies the JORF passes write from these articles: the unnumbered ones
    /// joined, and every stock article in its struct order.
    fn assemblies(&self) -> Vec<String> {
        let unnumbered = (!self.unnumbered.is_empty()).then(|| {
            let parts: Vec<&str> = self.unnumbered.values().map(String::as_str).collect();
            parts.join("\n\n")
        });
        let stock = (!self.stock.is_empty())
            .then(|| super::treaty_bodies::assemble_ordered(self.order.as_deref(), &self.stock));
        unnumbered
            .into_iter()
            .chain(stock)
            .map(|body| normalize_chars(&body))
            .collect()
    }
}

enum BodyOutcome {
    Current,
    Unmatched,
    Drifted(BodyTextWrite),
}

/// A stored body is rewritten when it holds an assembly's words in an older
/// layout; one that holds other words came from elsewhere and is left alone.
async fn rewrite_bodies(
    repo: &DecisionRepository<'_>,
    parts: HashMap<String, BodyParts>,
    spans: Option<Spans>,
    tally: &mut Tally,
) -> Result<()> {
    let uids: Vec<String> = parts.keys().cloned().collect();
    let parts = Arc::new(parts);
    for batch in uids.chunks(BATCH) {
        let stored = repo
            .legal_text_bodies_of(batch)
            .await
            .map_err(|e| anyhow!("legal_text_bodies_of: {e}"))?;
        tally.bodies_compared += stored.len();
        let parts = parts.clone();
        let outcomes: Vec<BodyOutcome> = tokio::task::spawn_blocking(move || {
            stored
                .into_par_iter()
                .map(|(text_uid, stored)| body_outcome(&parts[&text_uid], text_uid, stored, spans))
                .collect()
        })
        .await
        .map_err(|e| anyhow!("body spans join: {e}"))?;
        let mut writes = Vec::new();
        for outcome in outcomes {
            match outcome {
                BodyOutcome::Current => tally.bodies_current += 1,
                BodyOutcome::Unmatched => tally.bodies_unmatched += 1,
                BodyOutcome::Drifted(write) => writes.push(write),
            }
        }
        tally.bodies_drifted += writes.len();
        if spans.is_some() && !writes.is_empty() {
            tally.bodies_written += repo
                .rewrite_legal_text_bodies(&writes)
                .await
                .map_err(|e| anyhow!("rewrite_legal_text_bodies: {e}"))?;
        }
    }
    Ok(())
}

fn body_outcome(
    parts: &BodyParts,
    text_uid: String,
    stored: String,
    spans: Option<Spans>,
) -> BodyOutcome {
    let assemblies = parts.assemblies();
    if assemblies.contains(&stored) {
        return BodyOutcome::Current;
    }
    let Some(body) = assemblies
        .into_iter()
        .find(|body| words(body).eq(words(&stored)))
    else {
        tracing::debug!(%text_uid, "backfill-article-text: body matches no assembly");
        return BodyOutcome::Unmatched;
    };
    let OwnerRows { cases, cites } = match spans {
        Some(spans) => spans.rows(&body, &text_uid, None, None),
        None => OwnerRows {
            cases: Vec::new(),
            cites: Vec::new(),
        },
    };
    BodyOutcome::Drifted(BodyTextWrite {
        text_uid,
        stored,
        body,
        cases,
        cites,
    })
}

/// A text without the whitespace and cell separators the HTML reading lays out.
fn words(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().filter(|c| !c.is_whitespace() && *c != '|')
}
