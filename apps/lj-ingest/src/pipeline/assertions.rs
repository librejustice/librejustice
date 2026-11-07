//! The applicability compiler as a command: texts in, assertions out, one
//! transaction per text, replayable. And the three-term control that reads what
//! the compiler left.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;

use lj_norm::applicability::{
    self, bare_text_coverage, compile_article, coverage, frozen_rows, wording_fingerprint,
    ArticleDiagnostics, Assertion, Base, SourceVersion, OPEN_START,
};
use lj_norm::journal_rules::{self, matter_of};
use lj_store::repository::{
    AssertionWrite, BareTextCandidate, CompileCandidate, CompileStateWrite, CoverageRuleWrite,
    CoverageWrite, DecisionRepository, ExpressionWrite, VersionForCompile, VersionKeyRow,
};

use super::applicability_events::{events_of_text, EventStats};
use crate::config::Settings;

const BATCH: usize = 200;

/// What EUR-Lex and some curated corpora write as the end of a version without a term.
const NO_TERM: NaiveDate = NaiveDate::from_ymd_opt(9999, 12, 31).unwrap();

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub full: bool,
    pub days: i32,
    pub text_uid: Option<String>,
    pub dry_run: bool,
}

#[derive(Default)]
struct BareTotals {
    texts: usize,
    unchanged: usize,
    changed: usize,
    failed: usize,
    doubts: usize,
}

#[derive(Default)]
struct Totals {
    texts: usize,
    unchanged: usize,
    written: usize,
    failed: usize,
    versions: usize,
    assertions: usize,
    never_applicable: u32,
    closing_markers: u32,
    inverted: u32,
    conflicts: u32,
    dangling_events: u32,
    without_expression: usize,
    counters: usize,
    foreign_owner: usize,
    dangling_targets: usize,
    alsace_curation: usize,
    by_journal: BTreeMap<String, usize>,
}

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    use chrono::Datelike;
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).expect("valid date")
}

fn to_chrono(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32).expect("valid date")
}

fn source_start(d: NaiveDate) -> Option<jiff::civil::Date> {
    (d != to_chrono(OPEN_START)).then(|| to_jiff(d))
}

fn source_end(d: Option<NaiveDate>) -> Option<jiff::civil::Date> {
    d.filter(|d| *d < NO_TERM).map(to_jiff)
}

fn hash_field(h: &mut xxhash_rust::xxh3::Xxh3, s: &str) {
    h.update(s.as_bytes());
    h.update(b"\0");
}

/// What the compilation of a text depends on, hashed: its versions and the
/// matter of each article, the text's own place, the journal with the rules it
/// resolves on, and the compiler's rules.
fn text_fingerprint(
    versions: &[VersionKeyRow],
    cand: &CompileCandidate,
    journal_code: &str,
) -> i64 {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    h.update(&applicability::COMPILER_VERSION.to_le_bytes());
    h.update(&applicability::FINGERPRINT_VERSION.to_le_bytes());
    hash_field(&mut h, cand.default_scope.as_deref().unwrap_or(""));
    hash_field(&mut h, &cand.jurisdiction);
    hash_field(&mut h, journal_code);
    for r in journal_rules::rules_of(journal_code) {
        hash_field(&mut h, r.place);
        hash_field(&mut h, r.matter.unwrap_or("*"));
        hash_field(&mut h, r.regime.as_str());
        hash_field(&mut h, &r.since.map(|d| d.to_string()).unwrap_or_default());
        hash_field(&mut h, &r.until.map(|d| d.to_string()).unwrap_or_default());
        hash_field(&mut h, r.source);
    }
    let mut last_key = None;
    for v in versions {
        if last_key != Some(v.num_key.as_str()) {
            last_key = Some(v.num_key.as_str());
            let matter = matter_of(journal_code, &cand.text_uid, &v.num_key, &cand.title);
            hash_field(&mut h, matter.unwrap_or("*"));
        }
        hash_field(&mut h, &v.num_key);
        hash_field(&mut h, &v.date_debut.to_string());
        hash_field(
            &mut h,
            &v.date_fin.map(|d| d.to_string()).unwrap_or_default(),
        );
        hash_field(&mut h, &v.status);
        h.update(&v.content_checksum.to_le_bytes());
        hash_field(&mut h, &v.source);
        hash_field(&mut h, &v.translation);
        hash_field(&mut h, v.lang_original.as_deref().unwrap_or(""));
    }
    i64::from_ne_bytes(h.digest().to_ne_bytes())
}

/// The act as its journal printed it is authentic, a consolidation is official,
/// a text first written in another language ranks as its source declares.
fn expression_status(source: &str, translation: &str, lang_original: Option<&str>) -> &'static str {
    if lang_original.is_some_and(|l| l != "fr") {
        return match translation {
            "officiel" => "official",
            "automatique" => "automatic",
            _ => "translation",
        };
    }
    if source == "jorf" {
        "authentic"
    } else {
        "official"
    }
}

fn expressions_of(v: &VersionForCompile) -> Vec<ExpressionWrite> {
    let mut out = Vec::new();
    if let Some(texte) = v.texte.as_deref().filter(|t| !t.trim().is_empty()) {
        let (fp, len) = wording_fingerprint(texte);
        out.push(ExpressionWrite {
            language: "fr".to_string(),
            status: expression_status(&v.source, &v.translation, v.lang_original.as_deref()),
            fingerprint: i64::from_ne_bytes(fp.to_ne_bytes()),
            length: len as i32,
            original: false,
        });
    }
    if let (Some(orig), Some(lang)) = (
        v.texte_original.as_deref().filter(|t| !t.trim().is_empty()),
        v.lang_original.as_deref().filter(|l| *l != "fr"),
    ) {
        let (fp, len) = wording_fingerprint(orig);
        out.push(ExpressionWrite {
            language: lang.to_string(),
            status: "authentic",
            fingerprint: i64::from_ne_bytes(fp.to_ne_bytes()),
            length: len as i32,
            original: true,
        });
    }
    out
}

struct Compiled {
    rows: Vec<AssertionWrite>,
    coverage: Vec<CoverageWrite>,
    state: CompileStateWrite,
    diag: ArticleDiagnostics,
    events: EventStats,
    without_expression: usize,
}

/// The vocabulary the compiler checks its inputs against: every place, and
/// which ones have children.
struct Vocabulary {
    parent: HashMap<String, Option<String>>,
    has_children: BTreeSet<String>,
}

impl Vocabulary {
    fn new(places: Vec<(String, String, Option<String>)>) -> Self {
        let has_children = places.iter().filter_map(|(_, _, p)| p.clone()).collect();
        let parent = places.into_iter().map(|(u, _, p)| (u, p)).collect();
        Self {
            parent,
            has_children,
        }
    }

    fn ancestors<'a>(&'a self, place: &'a str) -> Vec<&'a str> {
        let mut out = Vec::new();
        let mut cur = self.parent.get(place).and_then(|p| p.as_deref());
        while let Some(p) = cur {
            out.push(p);
            cur = self.parent.get(p).and_then(|p| p.as_deref());
        }
        out
    }

    /// The places a journal's rules name all exist, and none sits on the path
    /// of another: an exclusion must be able to be an absence, so the excluded
    /// place is a sibling, never a child, of the others.
    fn check_rules(&self, journal_code: &str) -> Result<()> {
        let mut places: Vec<String> = journal_rules::rules_of(journal_code)
            .iter()
            .map(|r| r.place.to_string())
            .collect();
        places.extend(
            Base::of(journal_code, None, None)
                .spans()
                .iter()
                .map(|s| s.place.clone()),
        );
        places.sort();
        places.dedup();
        for place in &places {
            if !self.parent.contains_key(place) {
                bail!("journal {journal_code}: place {place} is not in the vocabulary");
            }
            for other in &places {
                if other != place && self.ancestors(place).contains(&other.as_str()) {
                    bail!("journal {journal_code}: {other} is on the path of {place}");
                }
            }
        }
        Ok(())
    }

    fn check_text_place(&self, place: &str) -> Result<()> {
        if !place.starts_with("geo:") || !self.parent.contains_key(place) {
            bail!("lieu du texte {place} hors vocabulaire");
        }
        if self.has_children.contains(place) {
            bail!("lieu du texte {place} a des enfants, un état y couvrirait tous ses lecteurs");
        }
        Ok(())
    }
}

/// Compiles one text: its assertions and the state to record.
fn compile_text(
    cand: &CompileCandidate,
    versions: &[VersionForCompile],
    journal_code: &str,
    fingerprint: i64,
) -> Result<Compiled> {
    let (events, event_stats) = events_of_text(versions);
    let mut by_key: BTreeMap<&str, Vec<&VersionForCompile>> = BTreeMap::new();
    for v in versions {
        by_key.entry(v.num_key.as_str()).or_default().push(v);
    }
    let mut rows = Vec::new();
    let mut compiled: Vec<Assertion> = Vec::new();
    let mut diag = ArticleDiagnostics::default();
    let mut never_applicable_keys = Vec::new();
    let mut without_expression = 0;
    for (num_key, vs) in &by_key {
        let source_versions: Vec<SourceVersion> = vs
            .iter()
            .map(|v| SourceVersion {
                num_key: v.num_key.clone(),
                start: source_start(v.date_debut),
                end: source_end(v.date_fin),
                status: v.status.clone(),
            })
            .collect();
        let article_events = events.get(*num_key).map(Vec::as_slice).unwrap_or(&[]);
        let matter = matter_of(journal_code, &cand.text_uid, num_key, &cand.title);
        let base = Base::of(journal_code, matter, cand.default_scope.as_deref());
        let (assertions, d) = compile_article(&source_versions, article_events, &base);
        if d.never_applicable > 0 {
            never_applicable_keys.push((*num_key).to_string());
        }
        diag.never_applicable += d.never_applicable;
        diag.closing_markers += d.closing_markers;
        diag.inverted += d.inverted;
        diag.conflicts += d.conflicts;
        diag.dangling_events += d.dangling_events;
        compiled.extend(assertions.iter().cloned());
        for a in assertions {
            let version_start = to_chrono(a.version_start);
            let v = vs
                .iter()
                .find(|v| {
                    source_start(v.date_debut)
                        .map(to_chrono)
                        .unwrap_or(to_chrono(OPEN_START))
                        == version_start
                })
                .ok_or_else(|| {
                    anyhow!("{num_key} : le compilateur nomme une version absente, {version_start}")
                })?;
            let expressions = expressions_of(v);
            if expressions.is_empty() {
                without_expression += 1;
            }
            rows.push(AssertionWrite {
                num_key: (*num_key).to_string(),
                places: a.places,
                rule: a.rule,
                starts_on: to_chrono(a.start),
                ends_on: a.end.map(to_chrono),
                start_origin: a.start_origin.as_str(),
                end_origin: a.end_origin.map(|o| o.as_str()),
                start_source: a.start_source,
                end_source: a.end_source,
                version_start: v.date_debut,
                expressions,
            });
        }
    }
    let state = CompileStateWrite {
        text_uid: cand.text_uid.clone(),
        journal_code: journal_code.to_string(),
        nature: Some(cand.nature.clone()),
        fingerprint,
        versions: versions.len() as i32,
        assertions: rows.len() as i32,
        never_applicable: diag.never_applicable as i32,
        diagnostics: serde_json::json!({
            "closing_markers": diag.closing_markers,
            "inverted": diag.inverted,
            "conflicts": diag.conflicts,
            "dangling_events": diag.dangling_events,
            "without_expression": without_expression,
            "never_applicable_keys": never_applicable_keys,
            "counters": event_stats.counters,
            "foreign_owner": event_stats.foreign_owner,
            "dangling_targets": event_stats.dangling_targets,
            "alsace_curation": event_stats.alsace_curation,
        }),
    };
    let frozen = frozen_rows(journal_code, &compiled, |k| {
        matter_of(journal_code, &cand.text_uid, k, &cand.title)
    });
    compiled.extend(frozen);
    Ok(Compiled {
        rows,
        coverage: coverage_writes(&compiled)?,
        state,
        diag,
        events: event_stats,
        without_expression,
    })
}

/// A text's coverage as the store writes it.
fn coverage_writes(assertions: &[Assertion]) -> Result<Vec<CoverageWrite>> {
    Ok(coverage(assertions)
        .ok_or_else(|| anyhow!("a rule key unknown to this binary"))?
        .into_iter()
        .map(|c| CoverageWrite {
            place: c.place,
            rule: c.rule,
            starts_on: to_chrono(c.start),
            ends_on: c.end.map(to_chrono),
        })
        .collect())
}

/// Every span of every rule key, for the mirror the SQL passes unfold on.
fn coverage_rules() -> Vec<CoverageRuleWrite> {
    journal_rules::all_key_spans()
        .flat_map(|(rule, spans)| {
            spans.iter().map(move |s| CoverageRuleWrite {
                rule: rule.to_string(),
                place: s.place.clone(),
                since: s.since.map(to_chrono),
                until: s.until.map(to_chrono),
                since_source: s.since_source.clone(),
                until_source: s.until_source.clone(),
            })
        })
        .collect()
}

pub async fn assertions_compile(opts: CompileOptions) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);

    let vocabulary = Vocabulary::new(repo.places().await?);
    let geo_labels = repo.geo_labels().await?;
    let named: Vec<(String, String, Option<String>, Vec<String>)> = applicability::journals()
        .iter()
        .map(|j| {
            (
                j.code.to_string(),
                j.label.to_string(),
                Some(j.publisher.to_string()),
                j.languages.iter().map(|l| l.to_string()).collect(),
            )
        })
        .collect();
    for j in &named {
        vocabulary.check_rules(&j.0)?;
    }
    if !opts.dry_run {
        repo.upsert_journals(&named).await?;
        repo.replace_coverage_rules(&coverage_rules()).await?;
    }
    let mut known_journals: BTreeSet<String> = named.iter().map(|j| j.0.clone()).collect();

    let candidates = repo
        .compile_candidates(opts.full, opts.days, opts.text_uid.as_deref())
        .await?;
    let mut totals = Totals {
        texts: candidates.len(),
        ..Default::default()
    };
    tracing::info!(
        texts = candidates.len(),
        full = opts.full,
        "assertions: compilation"
    );

    let mut logged = 0;
    for chunk in candidates.chunks(BATCH) {
        let uids: Vec<String> = chunk.iter().map(|c| c.text_uid.clone()).collect();
        let known = repo.compile_fingerprints(&uids).await?;
        let mut keys_by_text: HashMap<String, Vec<VersionKeyRow>> = HashMap::new();
        for k in repo.version_keys_for_compile(&uids).await? {
            keys_by_text.entry(k.text_uid.clone()).or_default().push(k);
        }
        for cand in chunk {
            let Some(keys) = keys_by_text.get(cand.text_uid.as_str()) else {
                continue;
            };
            let journal_code = applicability::journal_code(&cand.text_uid, &cand.jurisdiction);
            let fingerprint = text_fingerprint(keys, cand, &journal_code);
            if !opts.dry_run && known.get(&cand.text_uid) == Some(&fingerprint) {
                totals.unchanged += 1;
                continue;
            }
            let outcome = compile_one(
                &repo,
                &opts,
                &vocabulary,
                &geo_labels,
                &mut known_journals,
                cand,
                &journal_code,
                fingerprint,
                &mut totals,
            )
            .await;
            if let Err(e) = outcome {
                totals.failed += 1;
                tracing::error!(text_uid = %cand.text_uid, erreur = %e, "assertions: text failed");
            }
        }
        if totals.written / 20_000 > logged {
            logged = totals.written / 20_000;
            tracing::info!(
                written = totals.written,
                unchanged = totals.unchanged,
                failed = totals.failed,
                assertions = totals.assertions,
                "assertions: running"
            );
        }
    }
    let bare = compile_bare_texts(&repo, &opts, &vocabulary).await?;
    if opts.full && !opts.dry_run {
        let purged = repo.purge_orphan_wordings().await?;
        println!("  {purged} orphan drafts purged");
    }
    println!(
        "assertions : {} textes, {} inchangés, {} écrits, {} en échec, {} versions, {} assertions ; \
         {} jamais applicables, {} marqueurs d'abrogation, {} inversées, {} conflits, \
         {} événements irrecevables, {} états sans rédaction, {} compteurs, {} cibles hors fonds, \
         {} compteurs d'un autre texte, {} clauses positives en curation",
        totals.texts,
        totals.unchanged,
        totals.written,
        totals.failed,
        totals.versions,
        totals.assertions,
        totals.never_applicable,
        totals.closing_markers,
        totals.inverted,
        totals.conflicts,
        totals.dangling_events,
        totals.without_expression,
        totals.counters,
        totals.dangling_targets,
        totals.foreign_owner,
        totals.alsace_curation,
    );
    for (j, n) in &totals.by_journal {
        println!("  {j}: {n} textes");
    }
    println!(
        "texts without an article: {} texts, {} unchanged, {} changed, {} failed, \
         {} gone without a usable date",
        bare.texts, bare.unchanged, bare.changed, bare.failed, bare.doubts
    );
    if opts.dry_run {
        println!("dry-run: nothing is written");
    }
    if totals.failed + bare.failed > 0 {
        bail!(
            "assertions : {} textes en échec",
            totals.failed + bare.failed
        );
    }
    Ok(())
}

/// A text without an article covers its own window, so its coverage is
/// computed whole at every visit and written only when it moved.
async fn compile_bare_texts(
    repo: &DecisionRepository<'_>,
    opts: &CompileOptions,
    vocabulary: &Vocabulary,
) -> Result<BareTotals> {
    let candidates = repo
        .bare_text_candidates(opts.full, opts.days, opts.text_uid.as_deref())
        .await?;
    let mut totals = BareTotals {
        texts: candidates.len(),
        ..Default::default()
    };
    for chunk in candidates.chunks(1_000) {
        let uids: Vec<String> = chunk.iter().map(|c| c.text_uid.clone()).collect();
        let abrogations = repo.text_abrogations(&uids).await?;
        let stored = repo.stored_coverages(&uids).await?;
        let mut batch = Vec::new();
        for cand in chunk {
            let dates = abrogations
                .get(&cand.text_uid)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let (journal_code, computed) = match bare_coverage(cand, dates, vocabulary) {
                Ok(c) => c,
                Err(e) => {
                    totals.failed += 1;
                    tracing::error!(text_uid = %cand.text_uid, erreur = %e, "assertions: text failed");
                    continue;
                }
            };
            let rows = computed.unwrap_or_else(|| {
                totals.doubts += 1;
                Vec::new()
            });
            if opts.dry_run && opts.text_uid.is_some() {
                for r in &rows {
                    println!(
                        "  {} [{} -> {}]",
                        r.rule.as_deref().or(r.place.as_deref()).unwrap_or(""),
                        r.starts_on,
                        r.ends_on
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "ouvert".into())
                    );
                }
            }
            let nature = Some(cand.nature.clone());
            let unchanged = match stored.get(&cand.text_uid) {
                Some((j, n, s)) => *j == journal_code && *n == nature && *s == rows,
                None => rows.is_empty(),
            };
            if unchanged {
                totals.unchanged += 1;
                continue;
            }
            batch.push((cand.text_uid.clone(), journal_code, nature, rows));
        }
        if !opts.dry_run && !batch.is_empty() {
            repo.replace_coverages(&batch).await?;
        }
        totals.changed += batch.len();
    }
    tracing::info!(
        texts = totals.texts,
        changed = totals.changed,
        doubts = totals.doubts,
        "assertions: texts without an article"
    );
    Ok(totals)
}

/// The journal and the sorted coverage of a text without an article.
fn bare_coverage(
    cand: &BareTextCandidate,
    abrogations: &[NaiveDate],
    vocabulary: &Vocabulary,
) -> Result<(String, Option<Vec<CoverageWrite>>)> {
    if let Some(place) = cand.default_scope.as_deref() {
        vocabulary.check_text_place(place)?;
    }
    let journal_code = applicability::journal_code(&cand.text_uid, &cand.jurisdiction);
    let matter = matter_of(&journal_code, &cand.text_uid, "", &cand.title);
    let base = Base::of(&journal_code, matter, cand.default_scope.as_deref());
    let abrogations: Vec<jiff::civil::Date> = abrogations.iter().map(|d| to_jiff(*d)).collect();
    let computed = bare_text_coverage(
        cand.date_publi.map(to_jiff),
        cand.date_texte.map(to_jiff),
        &cand.status,
        &abrogations,
        &base,
    )
    .map(|c| {
        let mut rows: Vec<CoverageWrite> = c
            .into_iter()
            .map(|c| CoverageWrite {
                place: c.place,
                rule: c.rule,
                starts_on: to_chrono(c.start),
                ends_on: c.end.map(to_chrono),
            })
            .collect();
        rows.sort();
        rows
    });
    Ok((journal_code, computed))
}

#[allow(clippy::too_many_arguments)]
async fn compile_one(
    repo: &DecisionRepository<'_>,
    opts: &CompileOptions,
    vocabulary: &Vocabulary,
    geo_labels: &HashMap<String, String>,
    known_journals: &mut BTreeSet<String>,
    cand: &CompileCandidate,
    journal_code: &str,
    fingerprint: i64,
    totals: &mut Totals,
) -> Result<()> {
    if let Some(place) = cand.default_scope.as_deref() {
        vocabulary.check_text_place(place)?;
    }
    if !known_journals.contains(journal_code) {
        vocabulary.check_rules(journal_code)?;
        let iso = journal_code.trim_start_matches("JO-");
        let label = geo_labels
            .get(&format!("geo:{iso}"))
            .map(|l| format!("Journal officiel ({l})"))
            .unwrap_or_else(|| format!("Journal officiel ({iso})"));
        if !opts.dry_run {
            repo.upsert_journals(&[(
                journal_code.to_string(),
                label,
                None,
                vec!["fr".to_string()],
            )])
            .await?;
        }
        known_journals.insert(journal_code.to_string());
    }
    let versions = repo.versions_for_compile(&cand.text_uid).await?;
    let compiled = compile_text(cand, &versions, journal_code, fingerprint)?;
    totals.versions += versions.len();
    totals.assertions += compiled.rows.len();
    totals.never_applicable += compiled.diag.never_applicable;
    totals.closing_markers += compiled.diag.closing_markers;
    totals.inverted += compiled.diag.inverted;
    totals.conflicts += compiled.diag.conflicts;
    totals.dangling_events += compiled.diag.dangling_events;
    totals.without_expression += compiled.without_expression;
    totals.counters += compiled.events.counters;
    totals.foreign_owner += compiled.events.foreign_owner;
    totals.dangling_targets += compiled.events.dangling_targets;
    totals.alsace_curation += compiled.events.alsace_curation;
    *totals
        .by_journal
        .entry(journal_code.to_string())
        .or_default() += 1;
    if opts.dry_run {
        if opts.text_uid.is_some() {
            for r in &compiled.rows {
                println!(
                    "  {} {} [{} -> {}] v{} {}/{} {}",
                    r.num_key,
                    r.rule.clone().unwrap_or_else(|| format!("{:?}", r.places)),
                    r.starts_on,
                    r.ends_on
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "ouvert".into()),
                    r.version_start,
                    r.start_origin,
                    r.end_origin.unwrap_or("-"),
                    r.start_source.as_deref().unwrap_or("")
                );
            }
        }
        return Ok(());
    }
    repo.replace_assertions(&compiled.state, &compiled.rows, &compiled.coverage)
        .await?;
    totals.written += 1;
    Ok(())
}

/// The three-term control, overlap, gap and inversion, by place unfolded, plus
/// the versions no state carries. Reports, never fails the chain.
pub async fn assertions_control() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let c = repo.assertion_control().await?;
    let overlaps: i64 = c.overlaps.iter().map(|(_, n)| n).sum();
    let gaps: i64 = c.gaps.iter().map(|(_, n)| n).sum();
    // Only a hole between two live versions is a defect: an abrogation the
    // source later undoes and an exclusion are the model rendering the source.
    let source_holes: i64 = c
        .gaps
        .iter()
        .filter(|(famille, _)| famille == "trou de la source")
        .map(|(_, n)| n)
        .sum();
    if overlaps + source_holes + c.inversions > 0 {
        tracing::warn!(
            overlaps,
            trous_de_source = source_holes,
            trous = gaps,
            inversions = c.inversions,
            versions_sans_etat = c.versions_without_state,
            "assertions: check in default"
        );
    } else {
        tracing::info!(
            versions_sans_etat = c.versions_without_state,
            "assertions: check"
        );
    }
    println!(
        "check: {overlaps} overlaps, {gaps} gaps, {} inversions, {} versions without a state",
        c.inversions, c.versions_without_state
    );
    for (source, n) in &c.overlaps {
        println!("  chevauchement {source}: {n}");
    }
    for (famille, n) in &c.gaps {
        println!("  gap, {famille}: {n}");
    }
    Ok(())
}

/// Rebuilds the corpus-of-a-place index from the assertions already written,
/// by the computation the compiler runs. The nature goes first: a text is only
/// ever covered once it names one.
pub async fn assertions_coverage() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let natures = repo.backfill_compile_state_natures().await?;
    tracing::info!(natures, "assertions: natures carried onto the states");
    repo.replace_coverage_rules(&coverage_rules()).await?;
    let texts = repo.compiled_texts().await?;
    let (mut keyed, mut named) = (0usize, 0usize);
    for chunk in texts.chunks(500) {
        let uids: Vec<String> = chunk.iter().map(|t| t.0.clone()).collect();
        let mut stored = repo.stored_assertions(&uids).await?;
        let mut batch = Vec::with_capacity(chunk.len());
        for (text_uid, journal_code, nature, title) in chunk {
            let mut assertions = stored.remove(text_uid).unwrap_or_default();
            let frozen = frozen_rows(journal_code, &assertions, |k| {
                matter_of(journal_code, text_uid, k, title)
            });
            assertions.extend(frozen);
            let rows = coverage_writes(&assertions)?;
            keyed += rows.iter().filter(|c| c.rule.is_some()).count();
            named += rows.iter().filter(|c| c.place.is_some()).count();
            batch.push((text_uid.clone(), journal_code.clone(), nature.clone(), rows));
        }
        repo.replace_coverages(&batch).await?;
    }
    tracing::info!(
        texts = texts.len(),
        keyed,
        named,
        "assertions: coverage rebuilt"
    );
    println!(
        "coverage: {natures} natures, {} texts, {keyed} keyed intervals, {named} intervals by place",
        texts.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(num_key: &str, checksum: i64) -> VersionKeyRow {
        VersionKeyRow {
            text_uid: "T".to_string(),
            num_key: num_key.to_string(),
            date_debut: NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            date_fin: None,
            status: "VIGUEUR".to_string(),
            source: "legifrance".to_string(),
            translation: "officiel".to_string(),
            content_checksum: checksum,
            lang_original: None,
        }
    }

    fn cand(jurisdiction: &str, default_scope: Option<&str>) -> CompileCandidate {
        CompileCandidate {
            text_uid: "T".to_string(),
            jurisdiction: jurisdiction.to_string(),
            default_scope: default_scope.map(str::to_string),
            nature: "CODE".to_string(),
            title: "Décret relatif aux marchés".to_string(),
        }
    }

    fn vocabulary(alsace_parent: &str) -> Vocabulary {
        let child =
            |uid: &str, parent: &str| (uid.to_string(), uid.to_string(), Some(parent.to_string()));
        Vocabulary::new(vec![
            ("geo:FR".to_string(), "France".to_string(), None),
            child("geo:FR-M", "geo:FR"),
            child("geo:FR-ALS", alsace_parent),
            child("geo:GP", "geo:FR"),
            child("geo:MQ", "geo:FR"),
            child("geo:GF", "geo:FR"),
            child("geo:RE", "geo:FR"),
            child("geo:YT", "geo:FR"),
            child("geo:BL", "geo:FR"),
            child("geo:MF", "geo:FR"),
            child("geo:PM", "geo:FR"),
            child("geo:PF", "geo:FR"),
            child("geo:NC", "geo:FR"),
            child("geo:WF", "geo:FR"),
            child("geo:TF", "geo:FR"),
        ])
    }

    #[test]
    fn the_fingerprint_moves_with_every_input_of_the_compilation() {
        let base = text_fingerprint(&[key("1", 1)], &cand("FR", None), "JORF");
        assert_eq!(
            base,
            text_fingerprint(&[key("1", 1)], &cand("FR", None), "JORF")
        );
        assert_ne!(
            base,
            text_fingerprint(&[key("1", 2)], &cand("FR", None), "JORF")
        );
        assert_ne!(
            base,
            text_fingerprint(&[key("1", 1)], &cand("NC", None), "JONC")
        );
        assert_ne!(
            base,
            text_fingerprint(&[key("1", 1)], &cand("FR", Some("geo:FR-ALS")), "JORF")
        );
        let mut translated = key("1", 1);
        translated.lang_original = Some("nl".to_string());
        assert_ne!(
            base,
            text_fingerprint(&[translated], &cand("FR", None), "JORF")
        );
        assert_ne!(
            base,
            text_fingerprint(&[key("1", 1)], &cand("FR", None), "JOUE")
        );
        let mut taxed = cand("FR", None);
        taxed.title = "Décret relatif à la taxe d'apprentissage".to_string();
        assert_ne!(base, text_fingerprint(&[key("1", 1)], &taxed, "JORF"));
    }

    #[test]
    fn a_consolidation_is_official_and_the_journal_is_authentic() {
        assert_eq!(
            expression_status("legifrance", "officiel", None),
            "official"
        );
        assert_eq!(expression_status("kali", "officiel", None), "official");
        assert_eq!(expression_status("jorf", "officiel", None), "authentic");
        assert_eq!(
            expression_status("legal-corpus", "non_officiel", None),
            "official"
        );
        assert_eq!(
            expression_status("legal-corpus", "non_officiel", Some("nl")),
            "translation"
        );
        assert_eq!(
            expression_status("legal-corpus", "officiel", Some("nl")),
            "official"
        );
        assert_eq!(
            expression_status("legal-corpus", "automatique", Some("ar")),
            "automatic"
        );
    }

    #[test]
    fn the_rules_refuse_a_place_on_the_path_of_another() {
        let siblings = vocabulary("geo:FR");
        siblings.check_rules("JORF").unwrap();
        assert!(siblings.check_text_place("geo:FR").is_err());
        assert!(siblings.check_text_place("geo:FR-ALS").is_ok());
        assert!(siblings.check_text_place("communaute:OHADA").is_err());
        assert!(vocabulary("geo:FR-M").check_rules("JORF").is_err());
    }
}
