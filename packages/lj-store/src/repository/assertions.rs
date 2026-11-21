//! Applicability assertions: what the compiler writes, what the controls and
//! the readers relate. One state per row, a list of places or the key of a
//! rule, a half-open window, the version whose wording it carries.

use std::collections::{BTreeMap, HashMap};

use super::support::legal_article_row_from_row;
use super::types::{LegalArticleRow, PlaceReading, PlacedElsewhere};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use chrono::{Datelike, NaiveDate};
use lj_norm::applicability::{unfold, Assertion, Origin};
use tokio_postgres::types::ToSql;

/// One instrument of a place's corpus, with the size of the group it sits in.
#[derive(Debug, Clone)]
pub struct PlaceCorpusRow {
    pub journal_code: String,
    pub nature: String,
    pub total: i64,
    pub title: String,
    pub slug: Option<String>,
    pub jurisdiction: String,
}

/// A text the compiler has to look at.
#[derive(Debug, Clone)]
pub struct CompileCandidate {
    pub text_uid: String,
    pub jurisdiction: String,
    pub default_scope: Option<String>,
    pub nature: String,
    pub title: String,
}

/// A French text without an article, with what dates it.
#[derive(Debug, Clone)]
pub struct BareTextCandidate {
    pub text_uid: String,
    pub jurisdiction: String,
    pub default_scope: Option<String>,
    pub nature: String,
    pub title: String,
    pub date_publi: Option<NaiveDate>,
    pub date_texte: Option<NaiveDate>,
    pub status: String,
}

/// A text's stored coverage with the group it is filed under.
pub type StoredCoverage = (String, Option<String>, Vec<CoverageWrite>);

/// What decides the compilation of a version, its text left out.
#[derive(Debug, Clone)]
pub struct VersionKeyRow {
    pub text_uid: String,
    pub num_key: String,
    pub date_debut: NaiveDate,
    pub date_fin: Option<NaiveDate>,
    pub status: String,
    pub source: String,
    pub translation: String,
    pub content_checksum: i64,
    pub lang_original: Option<String>,
}

/// A source version with what the compiler reads of it: the window, the
/// status, the wording and its original, the counter prose.
#[derive(Debug, Clone)]
pub struct VersionForCompile {
    pub text_uid: String,
    pub num: String,
    pub num_key: String,
    pub date_debut: NaiveDate,
    pub date_fin: Option<NaiveDate>,
    pub status: String,
    pub source: String,
    pub translation: String,
    pub content_checksum: i64,
    pub texte: Option<String>,
    pub texte_original: Option<String>,
    pub lang_original: Option<String>,
}

/// An assertion to write, with its expressions.
#[derive(Debug, Clone)]
pub struct AssertionWrite {
    pub num_key: String,
    pub places: Vec<String>,
    pub rule: Option<String>,
    pub starts_on: NaiveDate,
    pub ends_on: Option<NaiveDate>,
    pub start_origin: &'static str,
    pub end_origin: Option<&'static str>,
    pub start_source: Option<String>,
    pub end_source: Option<String>,
    pub version_start: NaiveDate,
    pub expressions: Vec<ExpressionWrite>,
}

#[derive(Debug, Clone)]
pub struct ExpressionWrite {
    pub language: String,
    pub status: &'static str,
    pub fingerprint: i64,
    pub length: i32,
    /// The wording's bytes live in `texte_original` rather than `texte`.
    pub original: bool,
}

/// One interval of a text's coverage, on a place or under the key of a rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoverageWrite {
    pub place: Option<String>,
    pub rule: Option<String>,
    pub starts_on: NaiveDate,
    pub ends_on: Option<NaiveDate>,
}

/// A span of a rule key, as the mirror the SQL passes unfold on holds it.
#[derive(Debug, Clone)]
pub struct CoverageRuleWrite {
    pub rule: String,
    pub place: String,
    pub since: Option<NaiveDate>,
    pub until: Option<NaiveDate>,
    pub since_source: String,
    pub until_source: String,
}

/// The state a text's compilation leaves.
#[derive(Debug, Clone)]
pub struct CompileStateWrite {
    pub text_uid: String,
    pub journal_code: String,
    pub nature: Option<String>,
    pub fingerprint: i64,
    pub versions: i32,
    pub assertions: i32,
    pub never_applicable: i32,
    pub diagnostics: serde_json::Value,
}

/// An assertion as read for an article, its key unfolded into places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionRow {
    pub id: i64,
    pub places: Vec<String>,
    pub starts_on: NaiveDate,
    pub ends_on: Option<NaiveDate>,
    pub start_origin: String,
    pub end_origin: Option<String>,
    pub start_source: Option<String>,
    pub end_source: Option<String>,
    pub version_start: NaiveDate,
}

/// An article of the fund: its text and its key.
pub type ArticleId = (String, String);

/// A version's window, `date_fin` inclusive as the law-at-date reads it.
pub type VersionWindow = (Option<NaiveDate>, Option<NaiveDate>);

/// Counts of the three-term control, ventilated by the source of the version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssertionControl {
    pub overlaps: Vec<(String, i64)>,
    pub gaps: Vec<(String, i64)>,
    pub inversions: i64,
    pub versions_without_state: i64,
}

impl DecisionRepository<'_> {
    pub async fn upsert_journals(
        &self,
        rows: &[(String, String, Option<String>, Vec<String>)],
    ) -> Result<()> {
        for (code, label, publisher, languages) in rows {
            self.conn
                .execute(
                    "INSERT INTO journal (code, label, publisher, languages) VALUES ($1, $2, $3, $4)
                     ON CONFLICT (code) DO UPDATE SET label = EXCLUDED.label,
                       publisher = EXCLUDED.publisher, languages = EXCLUDED.languages",
                    &[code, label, publisher, languages],
                )
                .await?;
        }
        Ok(())
    }

    pub async fn geo_labels(&self) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, label FROM facet_value WHERE facet = 'geo'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Ancestors of a place, the place first: the path a reader resolves on.
    pub async fn place_path(&self, place: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "WITH RECURSIVE up AS (
                   SELECT uid, parent_uid, 0 AS depth FROM facet_value WHERE uid = $1 AND facet = 'geo'
                   UNION ALL
                   SELECT f.uid, f.parent_uid, up.depth + 1 FROM facet_value f
                   JOIN up ON f.uid = up.parent_uid)
                 SELECT uid FROM up ORDER BY depth",
                &[&place],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Texts the compiler looks at: every text with articles when `full`,
    /// otherwise those never compiled or modified within `days`, plus a fifth
    /// of the fund by weekday, `last_modified` being a source date that a
    /// rewrite of the articles does not move.
    pub async fn compile_candidates(
        &self,
        full: bool,
        days: i32,
        only: Option<&str>,
    ) -> Result<Vec<CompileCandidate>> {
        let rows = match only {
            Some(uid) => {
                self.conn
                    .query(
                        "SELECT text_uid, jurisdiction, default_scope, nature, title FROM legal_text WHERE text_uid = $1",
                        &[&uid],
                    )
                    .await?
            }
            None if full => {
                self.conn
                    .query(
                        "SELECT t.text_uid, t.jurisdiction, t.default_scope, t.nature, t.title FROM legal_text t
                         WHERE EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                         ORDER BY t.text_uid",
                        &[],
                    )
                    .await?
            }
            None => {
                self.conn
                    .query(
                        "SELECT t.text_uid, t.jurisdiction, t.default_scope, t.nature, t.title FROM legal_text t
                         WHERE EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                           AND (NOT EXISTS (SELECT 1 FROM assertion_compile_state s WHERE s.text_uid = t.text_uid)
                                OR t.last_modified >= CURRENT_DATE - $1::int
                                OR abs(hashtext(t.text_uid)) % 5 = EXTRACT(DOW FROM CURRENT_DATE)::int - 1)
                         ORDER BY t.text_uid",
                        &[&days],
                    )
                    .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| CompileCandidate {
                text_uid: r.get(0),
                jurisdiction: r.get(1),
                default_scope: r.get(2),
                nature: r.get(3),
                title: r.get(4),
            })
            .collect())
    }

    /// The French texts without an article to look at: all of them, one, or
    /// those the last `days` wrote or abrogated, plus the fifth of the stock
    /// whose weekday it is.
    pub async fn bare_text_candidates(
        &self,
        full: bool,
        days: i32,
        only: Option<&str>,
    ) -> Result<Vec<BareTextCandidate>> {
        const BARE: &str = "SELECT t.text_uid, t.jurisdiction, t.default_scope, t.nature, t.title,
                                   t.date_publi, t.date_texte, coalesce(t.status, '')
                            FROM legal_text t
                            WHERE t.jurisdiction = 'FR'
                              AND NOT EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)";
        let rows = match only {
            Some(uid) => {
                self.conn
                    .query(&format!("{BARE} AND t.text_uid = $1"), &[&uid])
                    .await?
            }
            None if full => {
                self.conn
                    .query(&format!("{BARE} ORDER BY t.text_uid"), &[])
                    .await?
            }
            None => {
                self.conn
                    .query(
                        &format!(
                            "{BARE} AND (t.last_modified >= CURRENT_DATE - $1::int
                                OR abs(hashtext(t.text_uid)) % 5 = EXTRACT(DOW FROM CURRENT_DATE)::int - 1
                                OR t.text_uid IN (
                                    SELECT l.target_text_uid FROM legal_link l
                                    JOIN legal_text o ON o.text_uid = l.owner_text_uid
                                    WHERE l.typelien = 'ABROGE' AND l.target_kind = 'texte'
                                      AND o.last_modified >= CURRENT_DATE - $1::int))
                             ORDER BY t.text_uid"
                        ),
                        &[&days],
                    )
                    .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| BareTextCandidate {
                text_uid: r.get(0),
                jurisdiction: r.get(1),
                default_scope: r.get(2),
                nature: r.get(3),
                title: r.get(4),
                date_publi: r.get(5),
                date_texte: r.get(6),
                status: r.get(7),
            })
            .collect())
    }

    /// The dates the `ABROGE` edges between a whole text and another one give,
    /// by text, whichever side holds the copy and whatever its direction: the
    /// abrogating article's version start, else the other text's publication.
    /// Only a date after the text's own start says that it is the abrogated one.
    pub async fn text_abrogations(
        &self,
        text_uids: &[String],
    ) -> Result<HashMap<String, Vec<NaiveDate>>> {
        let rows = self
            .conn
            .query(
                "SELECT l.target_text_uid,
                        CASE WHEN l.owner_num_key = '' THEN coalesce(o.date_publi, l.owner_date_debut)
                             ELSE l.owner_date_debut END
                 FROM legal_link l
                 LEFT JOIN legal_text o ON o.text_uid = l.owner_text_uid AND l.owner_num_key = ''
                 WHERE l.target_text_uid = ANY($1) AND l.typelien = 'ABROGE' AND l.target_kind = 'texte'
                 UNION ALL
                 SELECT l.owner_text_uid, coalesce(t.date_publi, l.target_date)
                 FROM legal_link l
                 LEFT JOIN legal_text t ON t.text_uid = l.target_text_uid
                 WHERE l.owner_text_uid = ANY($1) AND l.owner_num_key = '' AND l.typelien = 'ABROGE'
                   AND l.target_kind = 'texte' AND coalesce(t.date_publi, l.target_date) IS NOT NULL",
                &[&text_uids],
            )
            .await?;
        let mut out: HashMap<String, Vec<NaiveDate>> = HashMap::new();
        for r in &rows {
            out.entry(r.get(0)).or_default().push(r.get(1));
        }
        Ok(out)
    }

    pub async fn compile_fingerprints(&self, text_uids: &[String]) -> Result<HashMap<String, i64>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, fingerprint FROM assertion_compile_state WHERE text_uid = ANY($1)",
                &[&text_uids],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    pub async fn version_keys_for_compile(
        &self,
        text_uids: &[String],
    ) -> Result<Vec<VersionKeyRow>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num_key, date_debut, date_fin, status, source, translation,
                        content_checksum, lang_original
                 FROM legal_article WHERE text_uid = ANY($1)
                 ORDER BY text_uid, num_key, date_debut",
                &[&text_uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| VersionKeyRow {
                text_uid: r.get(0),
                num_key: r.get(1),
                date_debut: r.get(2),
                date_fin: r.get(3),
                status: r.get(4),
                source: r.get(5),
                translation: r.get(6),
                content_checksum: r.get(7),
                lang_original: r.get(8),
            })
            .collect())
    }

    /// Every version of one text with its text: read only for a text about to
    /// be compiled, a code holding hundreds of megabytes of wording.
    pub async fn versions_for_compile(&self, text_uid: &str) -> Result<Vec<VersionForCompile>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num, num_key, date_debut, date_fin, status, source, translation,
                        content_checksum, texte, texte_original, lang_original
                 FROM legal_article WHERE text_uid = $1
                 ORDER BY num_key, date_debut",
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| VersionForCompile {
                text_uid: r.get(0),
                num: r.get(1),
                num_key: r.get(2),
                date_debut: r.get(3),
                date_fin: r.get(4),
                status: r.get(5),
                source: r.get(6),
                translation: r.get(7),
                content_checksum: r.get(8),
                texte: r.get(9),
                texte_original: r.get(10),
                lang_original: r.get(11),
            })
            .collect())
    }

    /// Ids of the wordings named by `(language, fingerprint, length)`, created
    /// on the carrier when unknown. The first version to show a wording carries
    /// its bytes.
    async fn wording_ids(
        &self,
        text_uid: &str,
        keys: &[(String, i64, i32, String, NaiveDate, bool)],
    ) -> Result<HashMap<(String, i64, i32), i64>> {
        let mut out = HashMap::new();
        if keys.is_empty() {
            return Ok(out);
        }
        let version: i16 = lj_norm::applicability::FINGERPRINT_VERSION;
        for chunk in keys.chunks(200) {
            let mut sql = String::from(
                "INSERT INTO wording (language, fingerprint, fingerprint_version, length, \
                 carrier_text_uid, carrier_num_key, carrier_start, carrier_original) VALUES ",
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
            for (i, (lang, fp, len, num_key, start, original)) in chunk.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let b = i * 7;
                sql.push_str(&format!(
                    "(${},${},{version},${},${},${},${},${})",
                    b + 1,
                    b + 2,
                    b + 3,
                    b + 4,
                    b + 5,
                    b + 6,
                    b + 7
                ));
                params.push(lang);
                params.push(fp);
                params.push(len);
                params.push(&text_uid);
                params.push(num_key);
                params.push(start);
                params.push(original);
            }
            sql.push_str(
                " ON CONFLICT (language, fingerprint, fingerprint_version, length) DO NOTHING",
            );
            let _ = self.conn.execute(sql.as_str(), &params).await?;
            let langs: Vec<&String> = chunk.iter().map(|k| &k.0).collect();
            let fps: Vec<i64> = chunk.iter().map(|k| k.1).collect();
            let lens: Vec<i32> = chunk.iter().map(|k| k.2).collect();
            let rows = self
                .conn
                .query(
                    "SELECT w.id, w.language, w.fingerprint, w.length FROM wording w
                     JOIN unnest($1::text[], $2::bigint[], $3::int[]) AS k(language, fingerprint, length)
                       ON k.language = w.language AND k.fingerprint = w.fingerprint AND k.length = w.length
                     WHERE w.fingerprint_version = $4",
                    &[&langs, &fps, &lens, &version],
                )
                .await?;
            for r in rows {
                out.insert((r.get(1), r.get(2), r.get(3)), r.get(0));
            }
        }
        Ok(out)
    }

    /// Replaces every assertion of a text, expressions and coverage included,
    /// and records the compile state. One transaction: a reader sees the old
    /// rows or the new.
    pub async fn replace_assertions(
        &self,
        state: &CompileStateWrite,
        rows: &[AssertionWrite],
        coverage: &[CoverageWrite],
    ) -> Result<()> {
        let mut keys: Vec<(String, i64, i32, String, NaiveDate, bool)> = Vec::new();
        for a in rows {
            for e in &a.expressions {
                keys.push((
                    e.language.clone(),
                    e.fingerprint,
                    e.length,
                    a.num_key.clone(),
                    a.version_start,
                    e.original,
                ));
            }
        }
        keys.sort();
        keys.dedup_by(|a, b| (&a.0, a.1, a.2) == (&b.0, b.1, b.2));

        self.conn.batch_execute("BEGIN").await?;
        let result = self.write_assertions(state, rows, coverage, &keys).await;
        match result {
            Ok(()) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn write_assertions(
        &self,
        state: &CompileStateWrite,
        rows: &[AssertionWrite],
        coverage: &[CoverageWrite],
        keys: &[(String, i64, i32, String, NaiveDate, bool)],
    ) -> Result<()> {
        self.conn
            .execute(
                "SELECT pg_advisory_xact_lock(hashtext($1))",
                &[&state.text_uid],
            )
            .await?;
        let wordings = self.wording_ids(&state.text_uid, keys).await?;
        self.conn
            .execute(
                "DELETE FROM assertion WHERE text_uid = $1",
                &[&state.text_uid],
            )
            .await?;
        for chunk in rows.chunks(200) {
            let n_rows = chunk.len() as i64;
            let ids: Vec<i64> = self
                .conn
                .query(
                    "SELECT nextval(pg_get_serial_sequence('assertion', 'id')) FROM generate_series(1::bigint, $1)",
                    &[&n_rows],
                )
                .await?
                .iter()
                .map(|r| r.get(0))
                .collect();
            let mut sql = String::from(
                "INSERT INTO assertion (id, text_uid, num_key, places, rule, starts_on, ends_on, start_origin, \
                 end_origin, start_source, end_source, version_start) OVERRIDING SYSTEM VALUE VALUES ",
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
            for (i, (a, id)) in chunk.iter().zip(ids.iter()).enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let b = i * 12;
                sql.push_str(&format!(
                    "(${},${},${},${},${},${},${},${},${},${},${},${})",
                    b + 1,
                    b + 2,
                    b + 3,
                    b + 4,
                    b + 5,
                    b + 6,
                    b + 7,
                    b + 8,
                    b + 9,
                    b + 10,
                    b + 11,
                    b + 12
                ));
                params.push(id);
                params.push(&state.text_uid);
                params.push(&a.num_key);
                params.push(&a.places);
                params.push(&a.rule);
                params.push(&a.starts_on);
                params.push(&a.ends_on);
                params.push(&a.start_origin);
                params.push(&a.end_origin);
                params.push(&a.start_source);
                params.push(&a.end_source);
                params.push(&a.version_start);
            }
            self.conn.execute(sql.as_str(), &params).await?;
            let mut esql = String::from(
                "INSERT INTO expression (assertion_id, language, status, wording_id) VALUES ",
            );
            let mut eparams: Vec<Box<dyn ToSql + Sync + Send>> = Vec::new();
            let mut n = 0;
            for (a, id) in chunk.iter().zip(ids.iter()) {
                let id: i64 = *id;
                for e in &a.expressions {
                    let wid = wordings
                        .get(&(e.language.clone(), e.fingerprint, e.length))
                        .copied()
                        .expect("wording written before its expression");
                    if n > 0 {
                        esql.push(',');
                    }
                    let b = n * 4;
                    esql.push_str(&format!("(${},${},${},${})", b + 1, b + 2, b + 3, b + 4));
                    eparams.push(Box::new(id));
                    eparams.push(Box::new(e.language.clone()));
                    eparams.push(Box::new(e.status));
                    eparams.push(Box::new(wid));
                    n += 1;
                }
            }
            if n > 0 {
                let refs: Vec<&(dyn ToSql + Sync)> = eparams
                    .iter()
                    .map(|p| p.as_ref() as &(dyn ToSql + Sync))
                    .collect();
                self.conn.execute(esql.as_str(), &refs).await?;
            }
        }
        self.write_coverage(
            &state.text_uid,
            &state.journal_code,
            &state.nature,
            coverage,
        )
        .await?;
        self.conn
            .execute(
                "INSERT INTO assertion_compile_state (text_uid, journal_code, nature, fingerprint, compiled_at,
                   versions, assertions, never_applicable, diagnostics)
                 VALUES ($1, $2, $8, $3, now(), $4, $5, $6, $7)
                 ON CONFLICT (text_uid) DO UPDATE SET journal_code = EXCLUDED.journal_code,
                   nature = EXCLUDED.nature,
                   fingerprint = EXCLUDED.fingerprint, compiled_at = now(),
                   versions = EXCLUDED.versions, assertions = EXCLUDED.assertions,
                   never_applicable = EXCLUDED.never_applicable, diagnostics = EXCLUDED.diagnostics",
                &[
                    &state.text_uid,
                    &state.journal_code,
                    &state.fingerprint,
                    &state.versions,
                    &state.assertions,
                    &state.never_applicable,
                    &state.diagnostics,
                    &state.nature,
                ],
            )
            .await?;
        Ok(())
    }

    /// The merged intervals of a text's assertions, by place or under their
    /// key: what the corpus of a place reads instead of asking every text.
    async fn write_coverage(
        &self,
        text_uid: &str,
        journal_code: &str,
        nature: &Option<String>,
        coverage: &[CoverageWrite],
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM text_place_coverage WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        for chunk in coverage.chunks(500) {
            let mut sql = String::from(
                "INSERT INTO text_place_coverage (text_uid, journal_code, nature, place, rule, starts_on, ends_on) VALUES ",
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = vec![&text_uid, &journal_code, nature];
            for (i, c) in chunk.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let b = i * 4 + 3;
                sql.push_str(&format!(
                    "($1,$2,$3,${},${},${},${})",
                    b + 1,
                    b + 2,
                    b + 3,
                    b + 4
                ));
                params.push(&c.place);
                params.push(&c.rule);
                params.push(&c.starts_on);
                params.push(&c.ends_on);
            }
            self.conn.execute(sql.as_str(), &params).await?;
        }
        Ok(())
    }

    /// Rewrites the mirror of the rules the SQL passes unfold keys on.
    pub async fn replace_coverage_rules(&self, rows: &[CoverageRuleWrite]) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let result = self.write_coverage_rules(rows).await;
        match result {
            Ok(()) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn write_coverage_rules(&self, rows: &[CoverageRuleWrite]) -> Result<()> {
        self.conn.execute("DELETE FROM coverage_rule", &[]).await?;
        for chunk in rows.chunks(500) {
            let mut sql = String::from(
                "INSERT INTO coverage_rule (rule, place, since, until, since_source, until_source) VALUES ",
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
            for (i, r) in chunk.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let b = i * 6;
                sql.push_str(&format!(
                    "(${},${},${},${},${},${})",
                    b + 1,
                    b + 2,
                    b + 3,
                    b + 4,
                    b + 5,
                    b + 6
                ));
                params.push(&r.rule);
                params.push(&r.place);
                params.push(&r.since);
                params.push(&r.until);
                params.push(&r.since_source);
                params.push(&r.until_source);
            }
            self.conn.execute(sql.as_str(), &params).await?;
        }
        Ok(())
    }

    /// The three-term control on the unfolded view: a state that starts before
    /// the furthest end reached on its place overlaps, one that starts after it
    /// leaves a gap; inversions are the source windows the compiler set aside;
    /// and the versions that should carry a state and do not. Whole stock, so
    /// the pool's timeout is lifted for the transaction.
    ///
    /// Overlaps ventilate by the source of the version, gaps by what closed
    /// their left edge: an abrogation the source later undoes and an exclusion
    /// are the model working, only a hole between two live versions is a
    /// defect, and one undifferentiated number hid the three.
    pub async fn assertion_control(&self) -> Result<AssertionControl> {
        self.conn
            .batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
            .await?;
        let result = self.assertion_control_inner().await;
        let _ = self.conn.batch_execute("ROLLBACK").await;
        result
    }

    async fn assertion_control_inner(&self) -> Result<AssertionControl> {
        const REACHED: &str = "SELECT text_uid, num_key, place, version_start, starts_on,
                   max(coalesce(ends_on, 'infinity'::date)) OVER (
                     PARTITION BY text_uid, num_key, place ORDER BY starts_on
                     ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS reached
                 FROM assertion_by_place";
        let overlaps = self
            .conn
            .query(
                &format!(
                    "SELECT v.source, count(*) FROM ({REACHED}) x
                     JOIN legal_article v ON (v.text_uid, v.num_key, v.date_debut) = (x.text_uid, x.num_key, x.version_start)
                     WHERE x.reached > x.starts_on
                     GROUP BY 1 ORDER BY 2 DESC"
                ),
                &[],
            )
            .await?;
        let gaps = self
            .conn
            .query(
                &format!(
                    "SELECT CASE
                         WHEN EXISTS (SELECT 1 FROM legal_article v
                                      WHERE v.text_uid = x.text_uid AND v.num_key = x.num_key
                                        AND v.date_fin = x.reached - 1
                                        AND (v.status LIKE 'ABROGE%%' OR v.status IN
                                             ('TRANSFERE', 'DEPLACE', 'PERIME', 'ANNULE', 'REMPLACE', 'DENONCE')))
                              THEN 'abrogation puis rétablissement'
                         WHEN EXISTS (SELECT 1 FROM assertion_by_place a
                                      WHERE a.text_uid = x.text_uid AND a.num_key = x.num_key
                                        AND a.ends_on = x.reached AND a.place = x.place
                                        AND a.end_source IS NOT NULL)
                              THEN 'exclusion ou compteur refermé'
                         ELSE 'trou de la source' END AS famille,
                     count(*) FROM ({REACHED}) x
                     WHERE x.reached < x.starts_on
                     GROUP BY 1 ORDER BY 2 DESC"
                ),
                &[],
            )
            .await?;
        let inversions: i64 = self
            .conn
            .query_one(
                "SELECT coalesce(sum((diagnostics->>'inverted')::bigint), 0)::bigint
                 FROM assertion_compile_state",
                &[],
            )
            .await?
            .get(0);
        let without: i64 = self
            .conn
            .query_one(
                "SELECT count(*) FROM legal_article a
                 JOIN assertion_compile_state s ON s.text_uid = a.text_uid
                 WHERE a.status NOT IN ('MODIFIE_MORT_NE', 'DISJOINT')
                   AND a.date_debut < DATE '2222-01-01'
                   AND NOT (a.date_fin IS NOT NULL AND a.date_fin < a.date_debut)
                   AND NOT (a.date_fin IS NULL AND a.date_debut <> DATE '0001-01-01'
                            AND (a.status LIKE 'ABROGE%'
                                 OR a.status IN ('TRANSFERE', 'DEPLACE', 'PERIME', 'ANNULE', 'REMPLACE', 'DENONCE')))
                   AND NOT EXISTS (SELECT 1 FROM assertion x
                                   WHERE x.text_uid = a.text_uid AND x.num_key = a.num_key
                                     AND x.version_start = a.date_debut)",
                &[],
            )
            .await?
            .get(0);
        Ok(AssertionControl {
            overlaps: overlaps.iter().map(|r| (r.get(0), r.get(1))).collect(),
            gaps: gaps.iter().map(|r| (r.get(0), r.get(1))).collect(),
            inversions,
            versions_without_state: without,
        })
    }

    /// The states of an article as a reader meets them: a keyed row unfolded
    /// with the rules of this binary, then the rows that differ by the place
    /// alone merged back, each keeping the lowest id it merges.
    pub async fn assertions_of_article(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Vec<AssertionRow>> {
        let rows = self
            .conn
            .query(
                &format!("SELECT {STORED} FROM assertion WHERE text_uid = $1 AND num_key = $2"),
                &[&text_uid, &num_key],
            )
            .await?;
        type Merge = (
            NaiveDate,
            Option<NaiveDate>,
            NaiveDate,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let mut merged: BTreeMap<Merge, (i64, Vec<String>)> = BTreeMap::new();
        for r in &rows {
            let id: i64 = r.get(0);
            for a in unfolded(stored_assertion(num_key, r))? {
                let key = (
                    to_chrono(a.start),
                    a.end.map(to_chrono),
                    to_chrono(a.version_start),
                    a.start_origin.as_str().to_string(),
                    a.end_origin.map(|o| o.as_str().to_string()),
                    a.start_source,
                    a.end_source,
                );
                let entry = merged.entry(key).or_insert((id, Vec::new()));
                entry.0 = entry.0.min(id);
                entry.1.extend(a.places);
            }
        }
        let mut out: Vec<AssertionRow> = merged
            .into_iter()
            .map(
                |((starts_on, ends_on, version_start, so, eo, ss, es), (id, mut places))| {
                    places.sort();
                    places.dedup();
                    AssertionRow {
                        id,
                        places,
                        starts_on,
                        ends_on,
                        start_origin: so,
                        end_origin: eo,
                        start_source: ss,
                        end_source: es,
                        version_start,
                    }
                },
            )
            .collect();
        out.sort_by(|a, b| (a.starts_on, &a.places).cmp(&(b.starts_on, &b.places)));
        Ok(out)
    }

    /// Per article that has states, the places they cover on `day`, keyed
    /// rows unfolded: empty when none covers that day.
    pub async fn places_on(
        &self,
        articles: &[ArticleId],
        day: NaiveDate,
    ) -> Result<HashMap<ArticleId, Vec<String>>> {
        let (uids, keys): (Vec<&str>, Vec<&str>) = articles
            .iter()
            .map(|(u, k)| (u.as_str(), k.as_str()))
            .unzip();
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT {STORED}, text_uid, num_key FROM assertion
                     JOIN unnest($1::text[], $2::text[]) AS c(text_uid, num_key)
                       USING (text_uid, num_key)"
                ),
                &[&uids, &keys],
            )
            .await?;
        let on = to_jiff(day);
        let mut out: HashMap<ArticleId, Vec<String>> = HashMap::new();
        for r in &rows {
            let article: ArticleId = (r.get(10), r.get(11));
            let places = out.entry(article.clone()).or_default();
            for a in unfolded(stored_assertion(&article.1, r))? {
                if a.start <= on && a.end.is_none_or(|e| e > on) {
                    places.extend(a.places);
                }
            }
        }
        Ok(out)
    }

    /// Per article, the windows of its versions.
    pub async fn version_windows(
        &self,
        articles: &[ArticleId],
    ) -> Result<HashMap<ArticleId, Vec<VersionWindow>>> {
        let (uids, keys): (Vec<&str>, Vec<&str>) = articles
            .iter()
            .map(|(u, k)| (u.as_str(), k.as_str()))
            .unzip();
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num_key, date_debut, date_fin FROM legal_article
                 JOIN unnest($1::text[], $2::text[]) AS c(text_uid, num_key)
                   USING (text_uid, num_key)",
                &[&uids, &keys],
            )
            .await?;
        let mut out: HashMap<ArticleId, Vec<VersionWindow>> = HashMap::new();
        for r in &rows {
            out.entry((r.get(0), r.get(1)))
                .or_default()
                .push((r.get(2), r.get(3)));
        }
        Ok(out)
    }

    /// Every compiled text with the journal and the nature its coverage
    /// groups under.
    pub async fn compiled_texts(&self) -> Result<Vec<(String, String, Option<String>, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT s.text_uid, s.journal_code, s.nature, coalesce(t.title, '')
                 FROM assertion_compile_state s LEFT JOIN legal_text t ON t.text_uid = s.text_uid
                 ORDER BY s.text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// The stored assertions of some texts, as the compiler wrote them, keyed
    /// rows folded.
    pub async fn stored_assertions(
        &self,
        text_uids: &[String],
    ) -> Result<HashMap<String, Vec<Assertion>>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT {STORED}, text_uid, num_key FROM assertion WHERE text_uid = ANY($1)"
                ),
                &[&text_uids],
            )
            .await?;
        let mut out: HashMap<String, Vec<Assertion>> = HashMap::new();
        for r in &rows {
            let num_key: String = r.get(11);
            out.entry(r.get(10))
                .or_default()
                .push(stored_assertion(&num_key, r));
        }
        Ok(out)
    }

    /// The stored coverage of some texts, its rows sorted.
    pub async fn stored_coverages(
        &self,
        text_uids: &[String],
    ) -> Result<HashMap<String, StoredCoverage>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, journal_code, nature, place, rule, starts_on, ends_on
                 FROM text_place_coverage WHERE text_uid = ANY($1)",
                &[&text_uids],
            )
            .await?;
        let mut out: HashMap<String, StoredCoverage> = HashMap::new();
        for r in &rows {
            out.entry(r.get(0))
                .or_insert_with(|| (r.get(1), r.get(2), Vec::new()))
                .2
                .push(CoverageWrite {
                    place: r.get(3),
                    rule: r.get(4),
                    starts_on: r.get(5),
                    ends_on: r.get(6),
                });
        }
        for (_, _, rows) in out.values_mut() {
            rows.sort();
        }
        Ok(out)
    }

    /// Replaces the coverage of some texts, in one transaction.
    pub async fn replace_coverages(
        &self,
        texts: &[(String, String, Option<String>, Vec<CoverageWrite>)],
    ) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let result = self.write_coverages(texts).await;
        match result {
            Ok(()) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn write_coverages(
        &self,
        texts: &[(String, String, Option<String>, Vec<CoverageWrite>)],
    ) -> Result<()> {
        let uids: Vec<&str> = texts.iter().map(|t| t.0.as_str()).collect();
        self.conn
            .execute(
                "DELETE FROM text_place_coverage WHERE text_uid = ANY($1)",
                &[&uids],
            )
            .await?;
        let rows: Vec<(&String, &String, &Option<String>, &CoverageWrite)> = texts
            .iter()
            .flat_map(|(uid, journal, nature, coverage)| {
                coverage.iter().map(move |c| (uid, journal, nature, c))
            })
            .collect();
        for chunk in rows.chunks(500) {
            let mut sql = String::from(
                "INSERT INTO text_place_coverage (text_uid, journal_code, nature, place, rule, starts_on, ends_on) VALUES ",
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(chunk.len() * 7);
            for (i, (uid, journal, nature, c)) in chunk.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let b = i * 7;
                sql.push_str(&format!(
                    "(${},${},${},${},${},${},${})",
                    b + 1,
                    b + 2,
                    b + 3,
                    b + 4,
                    b + 5,
                    b + 6,
                    b + 7
                ));
                params.push(*uid);
                params.push(*journal);
                params.push(*nature);
                params.push(&c.place);
                params.push(&c.rule);
                params.push(&c.starts_on);
                params.push(&c.ends_on);
            }
            self.conn.execute(sql.as_str(), &params).await?;
        }
        Ok(())
    }

    /// Carries the nature of every compiled text onto its state, so the corpus
    /// groups without reading `legal_text`. Returns the rows touched.
    pub async fn backfill_compile_state_natures(&self) -> Result<u64> {
        self.conn
            .batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
            .await?;
        let result = self
            .conn
            .execute(
                "UPDATE assertion_compile_state s SET nature = t.nature
                 FROM legal_text t
                 WHERE t.text_uid = s.text_uid AND s.nature IS DISTINCT FROM t.nature",
                &[],
            )
            .await;
        match result {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e.into())
            }
        }
    }

    /// Wordings no expression names any more, left by recompiled texts.
    pub async fn purge_orphan_wordings(&self) -> Result<u64> {
        self.conn
            .batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
            .await?;
        let result = self
            .conn
            .execute(
                "DELETE FROM wording w WHERE NOT EXISTS (SELECT 1 FROM expression e WHERE e.wording_id = w.id)",
                &[],
            )
            .await;
        match result {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e.into())
            }
        }
    }

    /// The source version a state carries, by its start.
    pub async fn law_article_version(
        &self,
        text_uid: &str,
        num_key: &str,
        date_debut: NaiveDate,
    ) -> Result<Option<LegalArticleRow>> {
        let row = self
            .conn
            .query_opt(
                "SELECT text_uid, num, num_key, position, title_path, status,
                        date_debut, date_fin, texte, nota, content_checksum,
                        source, source_uid, source_url,
                        texte_original, lang_original, translation,
                        source_asof, source_upstream_url
                 FROM legal_article
                 WHERE text_uid = $1 AND num_key = $2 AND date_debut = $3",
                &[&text_uid, &num_key, &date_debut],
            )
            .await?;
        Ok(row.map(legal_article_row_from_row))
    }

    pub async fn place_labels(&self, uids: &[String]) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, label FROM facet_value WHERE facet = 'geo' AND uid = ANY($1)",
                &[&uids],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The whole `geo:*` vocabulary, roots first then children by parent.
    pub async fn places(&self) -> Result<Vec<(String, String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, label, parent_uid FROM facet_value WHERE facet = 'geo'
                 ORDER BY parent_uid NULLS FIRST, sort, uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Instruments whose states cover one of `path` at `date`, by place or by
    /// one of `keys`, the rule keys that reach the path that day: the corpus
    /// of a place, grouped by journal then nature. Every group carries its
    /// whole count and `per_group` of its titles, so a page never carries the
    /// metropolitan fund entire; a group that holds no more is carried whole,
    /// and the titles are sorted only among those shown.
    ///
    /// `count(*)` and not `count(DISTINCT text_uid)`, which costs forty times
    /// more: no text covers two places of one path, a place with children
    /// being refused at compile time, and a text covers under its key only
    /// when none of its places falls where the key reaches.
    pub async fn place_corpus(
        &self,
        path: &[String],
        keys: &[String],
        date: NaiveDate,
        per_group: i64,
    ) -> Result<Vec<PlaceCorpusRow>> {
        let rows = self
            .conn
            .query(
                "WITH hits AS (
                     SELECT journal_code, nature FROM text_place_coverage
                     WHERE place = ANY($1) AND starts_on <= $2
                       AND (ends_on IS NULL OR ends_on > $2)
                     UNION ALL
                     SELECT journal_code, nature FROM text_place_coverage
                     WHERE rule = ANY($4) AND starts_on <= $2
                       AND (ends_on IS NULL OR ends_on > $2)
                 ), groupes AS (
                     SELECT journal_code, nature, count(*) AS total FROM hits GROUP BY 1, 2
                 )
                 SELECT g.journal_code, g.nature, g.total, t.title, t.slug,
                        t.jurisdiction
                 FROM groupes g
                 CROSS JOIN LATERAL (
                     SELECT * FROM (
                         (SELECT lt.title, lt.slug, lt.jurisdiction
                          FROM text_place_coverage c
                          JOIN legal_text lt ON lt.text_uid = c.text_uid
                          WHERE c.place = ANY($1) AND c.journal_code = g.journal_code
                            AND c.nature = g.nature AND c.starts_on <= $2
                            AND (c.ends_on IS NULL OR c.ends_on > $2) AND lt.slug IS NOT NULL
                          LIMIT $3)
                         UNION ALL
                         (SELECT lt.title, lt.slug, lt.jurisdiction
                          FROM text_place_coverage c
                          JOIN legal_text lt ON lt.text_uid = c.text_uid
                          WHERE c.rule = ANY($4) AND c.journal_code = g.journal_code
                            AND c.nature = g.nature AND c.starts_on <= $2
                            AND (c.ends_on IS NULL OR c.ends_on > $2) AND lt.slug IS NOT NULL
                          LIMIT $3)
                     ) both_ways
                     LIMIT $3
                 ) t
                 ORDER BY g.journal_code, g.nature, t.title",
                &[&path, &date, &per_group, &keys],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| PlaceCorpusRow {
                journal_code: r.get(0),
                nature: r.get(1),
                total: r.get(2),
                title: r.get(3),
                slug: r.get(4),
                jurisdiction: r.get(5),
            })
            .collect())
    }

    /// The same corpus as [`Self::place_corpus`], as bare `text_uid`s and with
    /// no cap: a bench compares a whole set, a page the head of each group.
    pub async fn place_corpus_uids(
        &self,
        path: &[String],
        keys: &[String],
        date: NaiveDate,
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT c.text_uid FROM text_place_coverage c
                 WHERE (c.place = ANY($1) OR c.rule = ANY($3)) AND c.starts_on <= $2
                   AND (c.ends_on IS NULL OR c.ends_on > $2)",
                &[&path, &date, &keys],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The texts of `jurisdictions` the coverage places on `at`'s day somewhere
    /// else than `at`, under a key `at` drops or by name, and never there nor
    /// under a key `at` keeps. `journals` are those of the texts: naming them
    /// keeps the indexes in play, where a negation does not.
    pub async fn texts_placed_elsewhere(
        &self,
        at: &PlaceReading<'_>,
        jurisdictions: &[String],
        journals: &[String],
    ) -> Result<Vec<PlacedElsewhere>> {
        let rows = self
            .conn
            .query(
                "SELECT o.text_uid, o.jurisdiction, o.title, o.keyed,
                        EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = o.text_uid)
                 FROM (
                   SELECT c.text_uid, t.jurisdiction, coalesce(t.title, '') AS title,
                          bool_or(c.rule IS NOT NULL) AS keyed
                   FROM text_place_coverage c
                   JOIN legal_text t ON t.text_uid = c.text_uid AND t.jurisdiction = ANY($4)
                   WHERE (c.rule = ANY($6) OR (c.place IS NOT NULL AND c.place <> ALL($1)))
                     AND c.journal_code = ANY($5)
                     AND c.starts_on <= $2 AND (c.ends_on IS NULL OR c.ends_on > $2)
                     AND NOT EXISTS (
                       SELECT 1 FROM text_place_coverage h
                       WHERE h.text_uid = c.text_uid AND (h.place = ANY($1) OR h.rule = ANY($3))
                         AND h.starts_on <= $2 AND (h.ends_on IS NULL OR h.ends_on > $2))
                   GROUP BY c.text_uid, t.jurisdiction, t.title
                 ) o",
                &[
                    &at.path,
                    &at.day,
                    &at.keeping,
                    &jurisdictions,
                    &journals,
                    &at.dropping,
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| PlacedElsewhere {
                text_uid: r.get(0),
                jurisdiction: r.get(1),
                title: r.get(2),
                keyed: r.get(3),
                articles: r.get(4),
            })
            .collect())
    }

    /// The treaties with articles whose parties are known and leave `state` out.
    pub async fn treaties_without(&self, state: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid FROM legal_text t
                 WHERE t.jurisdiction = 'INTL' AND t.parties IS NOT NULL
                   AND $1 <> ALL(t.parties)
                   AND EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)",
                &[&state],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Other articles worded exactly like the version `(text_uid, num_key, start)`.
    pub async fn same_wording(
        &self,
        text_uid: &str,
        num_key: &str,
        version_start: NaiveDate,
    ) -> Result<Vec<(String, String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT a.text_uid, a.num_key, a.version_start
                 FROM expression e
                 JOIN assertion a ON a.id = e.assertion_id
                 WHERE e.wording_id IN (
                   SELECT e2.wording_id FROM expression e2
                   JOIN assertion a2 ON a2.id = e2.assertion_id
                   WHERE a2.text_uid = $1 AND a2.num_key = $2 AND a2.version_start = $3 AND e2.language = 'fr')
                   AND NOT (a.text_uid = $1 AND a.num_key = $2)
                 ORDER BY 1, 2, 3",
                &[&text_uid, &num_key, &version_start],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }
}

/// The states of an article covering a reader whose place resolves to `path`
/// at `day`: their places meet the path and their window holds the day. The
/// latest start first, then the lowest id; more than one is a conflict the
/// source left, shown rather than decided.
pub fn covering_states<'a>(
    rows: &'a [AssertionRow],
    path: &[String],
    day: NaiveDate,
) -> Vec<&'a AssertionRow> {
    let mut out: Vec<&AssertionRow> = rows
        .iter()
        .filter(|a| {
            a.places.iter().any(|p| path.contains(p))
                && a.starts_on <= day
                && a.ends_on.is_none_or(|e| e > day)
        })
        .collect();
    out.sort_by_key(|a| (std::cmp::Reverse(a.starts_on), a.id));
    out
}

/// The columns [`stored_assertion`] reads, in its order.
const STORED: &str = "id, places, rule, starts_on, ends_on, start_origin, end_origin, \
                      start_source, end_source, version_start";

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).expect("valid date")
}

fn to_chrono(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32).expect("valid date")
}

fn stored_assertion(num_key: &str, r: &tokio_postgres::Row) -> Assertion {
    let origin = |s: String| Origin::parse(&s).expect("origin checked by the schema");
    Assertion {
        num_key: num_key.to_string(),
        places: r.get(1),
        rule: r.get(2),
        start: to_jiff(r.get(3)),
        end: r.get::<_, Option<NaiveDate>>(4).map(to_jiff),
        start_origin: origin(r.get(5)),
        end_origin: r.get::<_, Option<String>>(6).map(origin),
        start_source: r.get(7),
        end_source: r.get(8),
        version_start: to_jiff(r.get(9)),
    }
}

/// A stored row as the named rows it stands for; a key this binary's rules
/// cannot unfold is an error.
fn unfolded(a: Assertion) -> Result<Vec<Assertion>> {
    unfold(&a).ok_or_else(|| {
        StoreError::Invalid(format!(
            "rule key {} unknown to this binary",
            a.rule.unwrap_or_default()
        ))
    })
}
