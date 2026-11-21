//! Rewrites what an unchanged DILA member now reads differently: article texte
//! and nota, and the spans indexing into them.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_extract_rows::{build_declared, owner_rows, Declared, OwnerRows};
use lj_pg::db::Connection;
use lj_sources::dila::DilaFond;
use lj_store::repository::{
    ArticleTextDrift, ArticleTextRead, ArticleTextWrite, DecisionRepository,
};
use rayon::prelude::*;

use super::jorf::{classify_jorf_member, JorfMember};
use super::kali::{classify_kali_member, KaliMember};
use super::legi::{classify_legi_member, LegiMember};
use super::ExtractCtx;
use crate::config::Settings;

/// Versions compared per round trip.
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
}

async fn backfill_fond(
    repo: &DecisionRepository<'_>,
    settings: &Settings,
    fond: ArticleFond,
    spans: Option<Spans>,
) -> Result<()> {
    let paths = lj_sources::dila::cached_dila_tarballs(&settings.cache_dir(), fond.dila())?;
    let mut tally = Tally {
        archives: paths.len(),
        ..Tally::default()
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Member>>(4);
    let reader = tokio::task::spawn_blocking(move || read_archives(fond, &paths, &tx));
    let mut reads: Vec<ArticleTextRead> = Vec::new();
    while let Some(members) = rx.recv().await {
        for member in members {
            tally.members += 1;
            match member {
                Member::Article(read) => reads.extend(read),
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
    tracing::warn!(
        fond = ?fond,
        dry_run = spans.is_none(),
        archives = tally.archives,
        members = tally.members,
        unreadable = tally.unreadable,
        versions_drifted = tally.drifted.len(),
        versions_written = tally.versions_written,
        "backfill-article-text"
    );
    Ok(())
}

/// What a member gives the pass.
enum Member {
    Article(Option<ArticleTextRead>),
    Unreadable,
}

/// Streams the archives in replay order, parsing each chunk of members in
/// parallel.
fn read_archives(
    fond: ArticleFond,
    paths: &[PathBuf],
    tx: &tokio::sync::mpsc::Sender<Vec<Member>>,
) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        let send = |chunk: Vec<(String, Vec<u8>)>| -> Result<()> {
            let members = chunk
                .into_par_iter()
                .map(|(name, raw)| read_member(fond, &name, &raw))
                .collect();
            tx.blocking_send(members)
                .map_err(|_| anyhow!("backfill-article-text: consumer stopped"))
        };
        let mut chunk: Vec<(String, Vec<u8>)> = Vec::with_capacity(CHUNK);
        lj_sources::tar_reader::for_each_member_filtered(
            path,
            |name| wanted(fond, name),
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

/// The members the pass reads: every article.
fn wanted(fond: ArticleFond, name: &str) -> bool {
    match fond {
        ArticleFond::Legi => matches!(classify_legi_member(name), LegiMember::Article),
        ArticleFond::Kali => matches!(classify_kali_member(name), KaliMember::Article),
        ArticleFond::Jorf => matches!(classify_jorf_member(name), JorfMember::Article),
    }
}

/// Reads a member as the ingest maps it: the versions it skips are skipped here.
fn read_member(fond: ArticleFond, name: &str, raw: &[u8]) -> Member {
    let checksum = xxhash_rust::xxh3::xxh3_64(raw);
    let member = match fond {
        ArticleFond::Legi => lj_extract::legi::parse_legi_article(raw).map(|a| {
            let placeholder = a
                .texte
                .as_deref()
                .is_some_and(lj_extract::legi::is_placeholder);
            Member::Article(
                (!a.num_key.is_empty() && !placeholder).then_some(ArticleTextRead {
                    source_uid: a.legiarti,
                    content_checksum: checksum,
                    texte: a.texte,
                    nota: a.nota,
                }),
            )
        }),
        ArticleFond::Kali => lj_extract::kali::parse_kali_article(raw).map(|a| {
            Member::Article((!a.num_key.is_empty()).then_some(ArticleTextRead {
                source_uid: a.kaliarti,
                content_checksum: checksum,
                texte: a.texte,
                nota: None,
            }))
        }),
        ArticleFond::Jorf => lj_extract::jorf::parse_jorf_article(raw).map(|a| {
            Member::Article(
                (a.num.is_some() && a.num_key.is_some()).then_some(ArticleTextRead {
                    source_uid: a.jorfarti,
                    content_checksum: checksum,
                    texte: a.texte,
                    nota: a.nota,
                }),
            )
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
