//! The parliamentary questions: each a text of the norms with its answers, its metadata a parliamentary document.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use serde_json::json;

use lj_parse::questions::{
    an_legislature, an_question, copy_header, copy_values, jo_issue, senat_answer, senat_question,
    Chamber, CopyRow, Printing, Question,
};
use lj_sources::questions::{AnSet, QuestionsClient, AN_REPOSITORY, DILA_AN, SENAT_DUMP};
use lj_store::repository::{BodyWrite, DecisionRepository, LegalTextRow, ParliamentDocumentRow};

use crate::config::Settings;

/// The Assemblée's legislatures in its open data, the last one current.
const AN_LEGISLATURES: [u16; 4] = [14, 15, 16, 17];
/// DILA's AN questions edition starts in October 2011.
const DILA_FIRST_YEAR: i32 = 2011;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum QuestionSource {
    An,
    Senat,
}

impl QuestionSource {
    fn label(self) -> &'static str {
        match self {
            QuestionSource::An => "an",
            QuestionSource::Senat => "senat",
        }
    }
}

/// The current legislature daily; `full` adds the closed ones and the answer pages DILA prints.
pub async fn sync_questions(
    source: Option<QuestionSource>,
    full: bool,
    dry_run: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = QuestionsClient::new(&settings.egress_proxy)
        .map_err(|e| anyhow!("questions client: {e}"))?;
    let dir = settings.cache_dir().join("questions");
    tokio::fs::create_dir_all(&dir).await?;
    let sources = match source {
        Some(s) => vec![s],
        None => vec![QuestionSource::An, QuestionSource::Senat],
    };
    let mut read: Vec<(QuestionSource, Vec<Question>)> = Vec::new();
    for source in sources {
        let questions = match source {
            QuestionSource::An => read_an(&client, &dir, full).await?,
            QuestionSource::Senat => read_senat(&client, &dir).await?,
        };
        report(source, &questions);
        read.push((source, questions));
    }
    if dry_run {
        return Ok(());
    }

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    for (source, questions) in &read {
        let known = repo
            .parliament_checksums(source.label())
            .await
            .map_err(|e| anyhow!("parliament_checksums: {e}"))?;
        let (mut written, mut undated) = (0, 0);
        for q in questions {
            let checksum = xxhash_rust::xxh3::xxh3_64(format!("{q:?}").as_bytes());
            if known.get(&q.uid) == Some(&checksum) {
                continue;
            }
            if write(&repo, *source, q, checksum).await? {
                written += 1;
            } else {
                undated += 1;
            }
        }
        tracing::info!(
            source = source.label(),
            read = questions.len(),
            written,
            undated,
            "sync_questions"
        );
    }
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(slugged, "sync_questions slugs");
    Ok(())
}

fn report(source: QuestionSource, questions: &[Question]) {
    let mut by: BTreeMap<(u16, &str), (usize, usize, usize)> = BTreeMap::new();
    for q in questions {
        let e = by.entry((q.legislature, q.kind.as_str())).or_default();
        e.0 += 1;
        if !q.answers.is_empty() {
            e.1 += 1;
            if q.answers.iter().all(|a| a.page.is_some()) {
                e.2 += 1;
            }
        }
    }
    for ((legislature, kind), (count, answered, paged)) in by {
        tracing::info!(
            source = source.label(),
            legislature,
            kind,
            count,
            answered,
            paged,
            "questions read"
        );
    }
}

async fn read_an(client: &QuestionsClient, dir: &Path, full: bool) -> Result<Vec<Question>> {
    let current = *AN_LEGISLATURES.last().expect("a legislature");
    let mut questions = Vec::new();
    let mut failed = 0;
    for legislature in AN_LEGISLATURES {
        if !full && legislature != current {
            continue;
        }
        for set in AnSet::ALL {
            let path = set.archive(legislature);
            let dest = dir.join(path.replace('/', "_"));
            client
                .file(&format!("{AN_REPOSITORY}/{path}"), &dest)
                .await
                .map_err(|e| anyhow!("{path}: {e}"))?;
            let (parsed, errors) = tokio::task::spawn_blocking(move || {
                let mut parsed = Vec::new();
                let mut errors = 0;
                lj_sources::questions::an_records(&dest, |record| match an_question(&record) {
                    Ok(q) => parsed.push(q),
                    Err(e) => {
                        errors += 1;
                        tracing::warn!(error = %e, "questions: record skipped");
                    }
                })
                .map(|_| (parsed, errors))
            })
            .await?
            .map_err(|e| anyhow!("{path}: {e}"))?;
            questions.extend(parsed);
            failed += errors;
        }
    }
    if full {
        let pages = dila_pages(client, dir).await?;
        let mut filled = 0;
        for q in &mut questions {
            for a in q.answers.iter_mut().filter(|a| a.page.is_none()) {
                if let Some(page) = a
                    .date
                    .zip(q.number.parse::<u32>().ok())
                    .and_then(|(d, n)| pages.get(&(q.legislature, n, d)))
                {
                    a.page = Some(*page);
                    filled += 1;
                }
            }
        }
        tracing::info!(filled, "questions: answer pages taken from DILA's issues");
    }
    if failed > 0 {
        tracing::warn!(failed, "questions: Assemblée records that did not parse");
    }
    Ok(questions)
}

/// The page of each answer DILA's issues print, by legislature, number and day.
async fn dila_pages(
    client: &QuestionsClient,
    dir: &Path,
) -> Result<HashMap<(u16, u32, jiff::civil::Date), u32>> {
    let this_year: i32 = chrono::Utc::now()
        .format("%Y")
        .to_string()
        .parse()
        .expect("a year");
    let mut folders: Vec<String> = (DILA_FIRST_YEAR..this_year)
        .map(|y| y.to_string())
        .collect();
    folders.push("Annee_en_cours".to_string());
    let mut pages = HashMap::new();
    let (mut issues, mut failed) = (0, 0);
    for folder in folders {
        let local = dir.join("dila-an").join(&folder);
        tokio::fs::create_dir_all(&local).await?;
        let listing = client
            .text(&format!("{DILA_AN}/{folder}/"))
            .await
            .map_err(|e| anyhow!("DILA {folder}: {e}"))?;
        for name in lj_sources::questions::dila_issue_names(&listing) {
            let cached = local.join(&name);
            let taz = match tokio::fs::read(&cached).await {
                Ok(taz) if folder != "Annee_en_cours" => taz,
                _ => {
                    let taz = client
                        .bytes(&format!("{DILA_AN}/{folder}/{name}"))
                        .await
                        .map_err(|e| anyhow!("DILA {name}: {e}"))?;
                    tokio::fs::write(&cached, &taz).await?;
                    taz
                }
            };
            let parsed = tokio::task::spawn_blocking(move || {
                lj_sources::questions::dila_issue_xml(&taz)
                    .map_err(|e| e.to_string())
                    .and_then(|xml| jo_issue(&xml))
            })
            .await?;
            let issue = match parsed {
                Ok(issue) => issue,
                Err(e) => {
                    failed += 1;
                    tracing::warn!(issue = %name, error = %e, "questions: DILA issue skipped");
                    continue;
                }
            };
            issues += 1;
            let Some(legislature) = an_legislature(issue.published) else {
                continue;
            };
            for answer in &issue.answers {
                for number in &answer.questions {
                    pages.insert((legislature, *number, issue.published), answer.page);
                }
            }
        }
    }
    tracing::info!(
        issues,
        failed,
        answers = pages.len(),
        "questions: DILA issues read"
    );
    Ok(pages)
}

async fn read_senat(client: &QuestionsClient, dir: &Path) -> Result<Vec<Question>> {
    let dest = dir.join("senat-questions.zip");
    client
        .file(SENAT_DUMP, &dest)
        .await
        .map_err(|e| anyhow!("Sénat dump: {e}"))?;
    tokio::task::spawn_blocking(move || -> Result<Vec<Question>> {
        let mut questions: BTreeMap<u64, Question> = BTreeMap::new();
        let mut table: Option<(String, Vec<String>)> = None;
        let (mut orphans, mut errors) = (0, 0);
        lj_sources::questions::senat_lines(&dest, |line| {
            if let Some(header) = copy_header(line) {
                table = Some(header);
                return;
            }
            if line.starts_with("\\.") {
                table = None;
                return;
            }
            let Some((name, columns)) = &table else {
                return;
            };
            let row = CopyRow {
                columns,
                values: copy_values(line),
            };
            match name.as_str() {
                "tam_questions" => match senat_question(&row) {
                    Ok((id, q)) => {
                        questions.insert(id, q);
                    }
                    Err(e) => {
                        errors += 1;
                        tracing::warn!(error = %e, "questions: Sénat question skipped");
                    }
                },
                "tam_reponses" => match senat_answer(&row) {
                    Ok((id, answer)) => match questions.get_mut(&id) {
                        Some(q) => q.answers.push(answer),
                        None => orphans += 1,
                    },
                    Err(e) => {
                        errors += 1;
                        tracing::warn!(error = %e, "questions: Sénat answer skipped");
                    }
                },
                _ => {}
            }
        })
        .map_err(|e| anyhow!("Sénat dump: {e}"))?;
        if orphans + errors > 0 {
            tracing::warn!(orphans, errors, "questions: Sénat rows left out");
        }
        Ok(questions.into_values().collect())
    })
    .await?
}

fn chrono_date(d: jiff::civil::Date) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(d.year().into(), d.month() as u32, d.day() as u32)
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "QE" => "Question écrite",
        "QG" => "Question au Gouvernement",
        "QOSD" => "Question orale sans débat",
        "QOAD" => "Question orale avec débat",
        "QOAE" => "Question orale européenne",
        "QC" => "Question cribles",
        _ => "Question",
    }
}

fn chamber_label(q: &Question) -> String {
    match q.chamber {
        Chamber::Assemblee => format!("Assemblée nationale, {}e législature", q.legislature),
        Chamber::Senat => "Sénat".to_string(),
    }
}

fn printed(p: &Printing) -> String {
    let mut at = String::new();
    if let Some(d) = p.date {
        at.push_str(&format!(", publiée au JO le {}", d.strftime("%d/%m/%Y")));
    }
    if let Some(page) = p.page {
        at.push_str(&format!(", page {page}"));
    }
    at
}

/// The question as printed, then each answer under its ministry and printing, each with its errata.
fn body(q: &Question) -> String {
    let mut parts = Vec::new();
    for asked in &q.asked {
        parts.push(format!("{}{}.", kind_label(&q.kind), printed(asked)));
        parts.push(asked.text.clone());
        if let Some(erratum) = &asked.erratum {
            parts.push(format!("Erratum : {erratum}"));
        }
    }
    for answer in &q.answers {
        let by = answer
            .ministry
            .as_deref()
            .map(|m| format!(" ({m})"))
            .unwrap_or_default();
        parts.push(format!("Réponse{by}{}.", printed(answer)));
        parts.push(answer.text.clone());
        if let Some(erratum) = &answer.erratum {
            parts.push(format!("Erratum : {erratum}"));
        }
    }
    parts.retain(|p| !p.trim().is_empty());
    parts.join("\n\n")
}

fn url(q: &Question) -> String {
    match q.chamber {
        Chamber::Assemblee => format!(
            "https://www.assemblee-nationale.fr/dyn/{}/questions/{}",
            q.legislature, q.uid
        ),
        Chamber::Senat => {
            let yy: u32 = q.uid.get(3..5).and_then(|y| y.parse().ok()).unwrap_or(0);
            let year = if yy >= 50 { 1900 + yy } else { 2000 + yy };
            format!("https://www.senat.fr/questions/base/{year}/q{}.html", q.uid)
        }
    }
}

/// Whether the question was written: one with no date at all is left out.
async fn write(
    repo: &DecisionRepository<'_>,
    source: QuestionSource,
    q: &Question,
    checksum: u64,
) -> Result<bool> {
    let asked = q.asked.first().and_then(|p| p.date).and_then(chrono_date);
    let answered = q
        .answers
        .iter()
        .filter_map(|a| a.date)
        .max()
        .and_then(chrono_date);
    let Some(dated) = asked.or(answered) else {
        return Ok(false);
    };
    let (chamber, journal_q, journal_debates) = match q.chamber {
        Chamber::Assemblee => ("an", "JO-QR-AN", "JO-DEBATS-AN"),
        Chamber::Senat => ("senat", "JO-QR-SENAT", "JO-DEBATS-SENAT"),
    };
    let author = q
        .author
        .name
        .as_deref()
        .map(|n| format!(" de {n}"))
        .unwrap_or_default();
    let subject = q
        .title
        .as_deref()
        .map(|t| format!(" : {t}"))
        .unwrap_or_default();
    let title = format!(
        "{} n° {}{author} ({}){subject}",
        kind_label(&q.kind),
        q.number,
        chamber_label(q)
    );
    let nature = if q.kind == "QE" {
        "QUESTION_ECRITE"
    } else {
        "QUESTION_ORALE"
    };
    repo.upsert_legal_text(&LegalTextRow {
        text_uid: q.uid.clone(),
        jurisdiction: "FR".to_string(),
        title_key: lj_extract::extract::normalize_instrument(&title),
        title,
        nature: nature.to_string(),
        last_modified: answered.or(asked),
        date_texte: Some(dated),
        date_publi: Some(dated),
        eli: None,
        nor: None,
        instrument_key: None,
        status: Some("VIGUEUR".to_string()),
    })
    .await
    .map_err(|e| anyhow!("upsert_legal_text {}: {e}", q.uid))?;
    repo.write_text_bodies("questions", &[(q.uid.clone(), body(q))], BodyWrite::Replace)
        .await
        .map_err(|e| anyhow!("write_text_bodies {}: {e}", q.uid))?;
    let printing = |edition: &str, journal: &str, p: &Printing| {
        json!({
            "edition": edition,
            "journal": journal,
            "date": p.date.map(|d| d.to_string()),
            "page": p.page,
            "ministry": p.ministry,
        })
    };
    let answer_journal = if q.kind == "QE" {
        journal_q
    } else {
        journal_debates
    };
    let printings: Vec<serde_json::Value> = q
        .asked
        .iter()
        .map(|p| printing("question", journal_q, p))
        .chain(
            q.answers
                .iter()
                .map(|p| printing("answer", answer_journal, p)),
        )
        .collect();
    repo.upsert_parliament_document(&ParliamentDocumentRow {
        uid: q.uid.clone(),
        chamber: chamber.to_string(),
        kind: q.kind.clone(),
        legislature: Some(q.legislature as i16),
        number: Some(q.number.clone()),
        title: q.title.clone(),
        analysis: q.rubric.clone(),
        authors: json!([{
            "name": q.author.name,
            "actor": q.author.actor,
            "group": q.author.group,
            "constituency": q.author.constituency,
        }]),
        ministry: q.ministry.clone(),
        dated: Some(dated),
        printings: serde_json::Value::Array(printings),
        url: Some(url(q)),
        fate: q.closure.clone(),
        text_uid: Some(q.uid.clone()),
        source: source.label().to_string(),
        source_uid: q.uid.clone(),
        content_checksum: checksum,
    })
    .await
    .map_err(|e| anyhow!("upsert_parliament_document {}: {e}", q.uid))?;
    Ok(true)
}
