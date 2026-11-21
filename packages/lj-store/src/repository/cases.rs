//! Case-law citations: one occurrence is one `case_citation` row
//! `(decision_id, char_start, char_end, address of the cited decision,
//! target_decision_id, extract_version)`. The address holds in three columns
//! `target_court`, `target_docket`, `target_date`; `target_decision_id` is set
//! by the SQL resolution (at write time, then relinked post-ingest, a target
//! being free to arrive after its citations).
//!
//! Written by the pattern of `citations.rs`: never-degrade guard, skip-diff of
//! unchanged sets (which preserves resolved targets), DELETE + binary COPY.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use super::support::address_columns;
use super::types::{CaseCitationRow, CitingDecisionRow};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Item d'écriture bulk : `(decision_id, citations)`. `None` = extraction
/// sans couche jurisprudence (écrit un set vide).
pub type CaseCitationWriteItem = (i64, Option<Vec<CaseCitationRow>>);

/// Fingerprint of an occurrence for the skip-diff: span + address aimed at.
/// `target_decision_id` is a resolution state rather than an extraction
/// output, so it never triggers a rewrite.
type CaseFingerprint = (i32, i32, Option<String>, Option<String>, Option<NaiveDate>);

/// Couverture chronologie d'un type de juridiction : `(type, décisions,
/// décisions portant au moins un lien amont, liens émis, liens résolus)`.
pub type ChronoCoverageRow = (String, i64, i64, i64, i64);

/// Clé du verrou advisory transactionnel de la sonde inversée
/// ([`DecisionRepository::attract_pending_citations`]) — hors de la plage des
/// ids de décisions (le verrou par décision d'`apply_provenance` partage le
/// même keyspace `pg_advisory_xact_lock(bigint)`).
const ATTRACT_LOCK_KEY: i64 = 0x6c6a_5f61_7474_7261; // « lj_attra »

impl DecisionRepository<'_> {
    /// (Ré)écrit les citations de jurisprudence d'UNE décision — sucre sur
    /// [`Self::replace_case_citations_bulk`].
    pub async fn replace_case_citations(
        &self,
        decision_id: i64,
        citations: Option<&[CaseCitationRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_case_citations_bulk(&[(decision_id, citations.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les citations de jurisprudence d'un lot de décisions : garde
    /// jamais-dégrader (version > `EXTRACT_VERSION` = révision manuelle, jamais
    /// remplacée), skip-diff des sets `(span, adresse)` inchangés
    /// (un set inchangé garde ses cibles résolues), puis DELETE + COPY binaire. Les
    /// lignes réécrites repartent pendantes (`target_decision_id` NULL) — la résolution
    /// par adresse les reprend. **La transaction appartient à l'appelant.** Idempotent.
    /// Rend les décisions RÉELLEMENT réécrites, pour que l'appelant date leur page du
    /// jour.
    #[tracing::instrument(name = "db.replace_case_citations_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_case_citations_bulk(
        &self,
        items: &[CaseCitationWriteItem],
    ) -> Result<Vec<i64>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let protected: HashSet<i64> = self
            .conn
            .query(
                "SELECT id FROM decisions WHERE id = ANY($1) AND extract_version > $2",
                &[&ids, &lj_versions::EXTRACT_VERSION],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();

        let db_rows = self
            .conn
            .query(
                "SELECT decision_id, char_start, char_end, target_court, target_docket,
                        target_date
                 FROM case_citation
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_versions::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<CaseFingerprint>> = HashMap::new();
        for row in &db_rows {
            current.entry(row.get(0)).or_default().insert((
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
                row.get(5),
            ));
        }
        let empty = HashSet::new();
        let changed: Vec<&CaseCitationWriteItem> = items
            .iter()
            .filter(|(id, citations)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<CaseFingerprint> = citations
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|c| {
                        let (court, docket, date) = address_columns(&c.address);
                        (
                            c.char_start,
                            c.char_end,
                            court.map(str::to_string),
                            docket.map(str::to_string),
                            date,
                        )
                    })
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(Vec::new());
        }

        for (decision_id, citations) in &changed {
            let mut prev_end = i32::MIN;
            for c in citations.as_deref().unwrap_or(&[]) {
                if c.char_start >= c.char_end || c.char_start < prev_end {
                    return Err(StoreError::Invalid(format!(
                        "case_citations invalides (décision {decision_id}) : span \
                         [{}, {}) après end={prev_end} — tri/chevauchement violé",
                        c.char_start, c.char_end
                    )));
                }
                prev_end = c.char_end;
            }
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM case_citation WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let has_rows = changed
            .iter()
            .any(|(_, c)| !c.as_deref().unwrap_or(&[]).is_empty());
        let sink = self
            .conn
            .copy_in(
                "COPY case_citation (decision_id, char_start, char_end, target_court, \
                 target_docket, target_date, extract_version) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT4,
                Type::INT4,
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::INT2,
            ],
        );
        tokio::pin!(writer);
        for (decision_id, citations) in &changed {
            for c in citations.as_deref().unwrap_or(&[]) {
                let (court, docket, date) = address_columns(&c.address);
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &c.char_start,
                        &c.char_end,
                        &court,
                        &docket,
                        &date,
                        &lj_versions::EXTRACT_VERSION,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        if has_rows {
            self.resolve_case_citations_scoped(Some(&changed_ids))
                .await?;
        }
        Ok(changed_ids)
    }

    /// Résout en batch toutes les citations pendantes dont la cible est en base
    /// (relink post-ingest / passe hebdo — une cible peut arriver après ses citations).
    /// Renvoie le nombre de citations résolues.
    #[tracing::instrument(name = "db.resolve_pending_case_citations", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_case_citations(&self) -> Result<u64> {
        self.resolve_case_citations_scoped(None).await
    }

    /// Résolution de l'adresse contre les `docket_numbers` des cibles (GIN),
    /// cour et date en filtre. Match unique → `target_decision_id` ;
    /// CJUE multi-match → snap au document ARRÊT (type CELEX `J` du `source_uid`,
    /// jamais les conclusions AG) ; ambigu ou introuvable → reste pendant, relinkable.
    /// `ids` = portée at-write (lignes fraîchement écrites), `None` = tout le pendant.
    async fn resolve_case_citations_scoped(&self, ids: Option<&[i64]>) -> Result<u64> {
        let scope = match ids {
            Some(_) => ResolveScope::ByDecision,
            None => ResolveScope::All,
        };
        let mut resolved = 0u64;
        for sql in resolve_case_address_sqls("case_citation", scope) {
            let n = match ids {
                Some(ids) => self.conn.execute(sql.as_str(), &[&ids]).await?,
                None => self.conn.execute(sql.as_str(), &[]).await?,
            };
            resolved += n;
        }
        Ok(resolved)
    }

    /// Resolution bounded to the dockets of an arriving batch of TARGET
    /// decisions (reverse probe). Same uniqueness as the sweep.
    async fn resolve_case_citations_for_arrival(&self, dockets: &[String]) -> Result<u64> {
        let mut resolved = 0u64;
        for sql in resolve_case_address_sqls("case_citation", ResolveScope::ByArrival) {
            resolved += self.conn.execute(sql.as_str(), &[&dockets]).await?;
        }
        Ok(resolved)
    }

    /// (Ré)écrit les citations de jurisprudence d'un TEXTE du référentiel : DELETE +
    /// COPY au grain `owner_text_uid` (corps + tous ses articles d'un coup —
    /// l'extraction passe le texte entier). Les lignes repartent pendantes ; la
    /// résolution se lance en fin de passe
    /// ([`Self::resolve_pending_text_case_citations`]). Idempotent.
    pub async fn replace_text_case_citations(
        &self,
        owner_text_uid: &str,
        rows: &[super::types::TextCaseCitationRow],
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM text_case_citation WHERE owner_text_uid = $1",
                &[&owner_text_uid],
            )
            .await?;
        let rows: Vec<_> = rows.iter().map(|r| (owner_text_uid, r)).collect();
        self.copy_text_case_citations(&rows).await
    }

    /// COPY of `text_case_citation` rows, each beside its owner `text_uid`.
    pub(super) async fn copy_text_case_citations(
        &self,
        rows: &[(&str, &super::types::TextCaseCitationRow)],
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let sink = self
            .conn
            .copy_in(
                "COPY text_case_citation (owner_text_uid, owner_num_key, owner_date_debut, \
                 char_start, char_end, target_court, target_docket, target_date, \
                 extract_version) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::INT4,
                Type::INT4,
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::INT2,
            ],
        );
        tokio::pin!(writer);
        for (owner_text_uid, r) in rows {
            let (court, docket, date) = address_columns(&r.address);
            writer
                .as_mut()
                .write(&[
                    &owner_text_uid,
                    &r.owner_num_key,
                    &r.owner_date_debut,
                    &r.char_start,
                    &r.char_end,
                    &court,
                    &docket,
                    &date,
                    &lj_versions::EXTRACT_VERSION,
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
    }

    /// Résout les `text_case_citation` pendantes : même résolveur que
    /// `case_citation`, colonnes cibles identiques, seul le nom de table change.
    #[tracing::instrument(name = "db.resolve_pending_text_case_citations", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_text_case_citations(&self) -> Result<u64> {
        let mut resolved = 0u64;
        for sql in resolve_case_address_sqls("text_case_citation", ResolveScope::All) {
            resolved += self.conn.execute(sql.as_str(), &[]).await?;
        }
        Ok(resolved)
    }

    /// Jumeau à l'arrivée de [`Self::resolve_pending_text_case_citations`],
    /// table `text_case_citation`.
    async fn resolve_text_case_citations_for_arrival(&self, dockets: &[String]) -> Result<u64> {
        let mut resolved = 0u64;
        for sql in resolve_case_address_sqls("text_case_citation", ResolveScope::ByArrival) {
            resolved += self.conn.execute(sql.as_str(), &[&dockets]).await?;
        }
        Ok(resolved)
    }

    /// Reverse probe of the pending rows, when a batch of TARGET decisions
    /// arrives: `dockets` are the numbers those decisions carry
    /// (`lj_extract::identity::pending_target_dockets`). Probes the partial GIN
    /// indexes of the three tables, then replays the resolvers only on the ones
    /// that answer: chronology links, then decision citations, then text
    /// citations. No hit (the common case) means no UPDATE. **Runs in the
    /// caller's transaction**; serialised between workers by a transactional
    /// advisory lock, so two crossing batches wait instead of deadlocking.
    /// Returns how many pending rows were resolved.
    #[tracing::instrument(name = "db.attract_pending_citations", skip_all, fields(db.system = "postgresql", dockets = dockets.len()))]
    pub async fn attract_pending_citations(&self, dockets: &[String]) -> Result<u64> {
        if dockets.is_empty() {
            return Ok(0);
        }
        let case_hit = self.has_pending_docket("case_citation", dockets).await?;
        let text_hit = self
            .has_pending_docket("text_case_citation", dockets)
            .await?;
        let link_hit = self.has_pending_docket("decision_links", dockets).await?;
        if !(case_hit || text_hit || link_hit) {
            return Ok(0);
        }
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&ATTRACT_LOCK_KEY])
            .await?;
        let mut resolved = 0u64;
        if link_hit {
            resolved += self.resolve_decision_links_for_arrival(dockets).await?;
        }
        if case_hit {
            resolved += self.resolve_case_citations_for_arrival(dockets).await?;
        }
        if text_hit {
            resolved += self
                .resolve_text_case_citations_for_arrival(dockets)
                .await?;
        }
        Ok(resolved)
    }

    /// Decisions whose text cites this one or whose Conseil d'Etat analysis
    /// refers to it, authority first then latest first, like the citing
    /// decisions of an article. A decision citing it several times comes out
    /// once. `exclues` takes out the steps of the chronology, this decision
    /// included. Returns `(total, head page)`.
    #[tracing::instrument(name = "db.decisions_citing", skip(self, exclues), fields(db.system = "postgresql", exclues = exclues.len()))]
    pub async fn decisions_citing(
        &self,
        decision_id: i64,
        exclues: &[i64],
        limite: i64,
        lang: &str,
    ) -> Result<(i64, Vec<CitingDecisionRow>)> {
        let rank = super::support::authority_rank_sql("d.publication_codes");
        let rows = self
            .conn
            .query(
                &format!(
                    "WITH citantes AS ( \
                         SELECT c.decision_id AS id FROM case_citation c \
                         WHERE c.target_decision_id = $1 \
                         UNION \
                         SELECT r.decision_id FROM decision_renvoi r \
                         WHERE r.target_decision_id = $1 \
                     ) \
                     SELECT d.id, d.public_id, d.jurisdiction_type, j.label, \
                            d.date_lecture::text, d.docket_numbers, \
                            d.publication_codes, {summary}, count(*) OVER (), \
                            d.jurisdiction_code, {bounds} AS summary_bounds \
                     FROM citantes ci \
                     JOIN decisions d ON d.id = ci.id AND d.deleted_at IS NULL \
                     LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                     WHERE NOT (d.id = ANY($2)) \
                     ORDER BY {rank}, d.date_lecture::text DESC NULLS LAST, d.id \
                     LIMIT $3",
                    summary = super::pick::summary("d", "$4"),
                    bounds = super::pick::summary_bounds("d", "$4")
                ),
                &[&decision_id, &exclues, &limite, &lang],
            )
            .await?;
        let total = rows.first().map_or(0, |r| r.get(8));
        Ok((
            total,
            rows.iter()
                .map(|r| {
                    let (summary_quote, summary_body) =
                        super::pick::bounds(r.get("summary_bounds"));
                    CitingDecisionRow {
                        id: r.get(0),
                        public_id: r.get(1),
                        jurisdiction_type: r.get(2),
                        jurisdiction_name: r.get(3),
                        jurisdiction_code: r.get("jurisdiction_code"),
                        date_lecture: r.get(4),
                        docket_numbers: r.get(5),
                        publication_codes: r.get(6),
                        summary: r.get(7),
                        summary_quote,
                        summary_body,
                        cited_num_keys: Vec::new(),
                    }
                })
                .collect(),
        ))
    }

    /// `source_uid` des décisions dont le greffe déclare lui-même la décision
    /// attaquée (`contested` complet). Cet oracle ne doit rien au texte, donc la
    /// voie texte se mesure contre lui sans circularité. Tri sur le `md5` du
    /// `source_uid` : échantillon stable d'un run à l'autre et non corrélé à la
    /// date, là où l'ordre naturel de l'uid Judilibre ne rendrait que les plus
    /// anciennes.
    #[tracing::instrument(name = "db.contested_oracle_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn contested_oracle_uids(
        &self,
        jurisdiction_type: &str,
        limite: i64,
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT s.source_uid FROM decisions d \
                 JOIN decision_sources s ON s.decision_id = d.id AND s.deleted_at IS NULL \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_type = $1 \
                   AND s.source_fields #>> '{contested,jurisdiction}' IS NOT NULL \
                   AND s.source_fields #>> '{contested,number}' IS NOT NULL \
                   AND s.source_fields #>> '{contested,date}' IS NOT NULL \
                 ORDER BY md5(s.source_uid) LIMIT $2",
                &[&jurisdiction_type, &limite],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Panneau de couverture du banc, par type de juridiction.
    #[tracing::instrument(name = "db.chrono_coverage", skip(self), fields(db.system = "postgresql"))]
    pub async fn chrono_coverage(
        &self,
        jurisdiction_types: &[String],
    ) -> Result<Vec<ChronoCoverageRow>> {
        let rows = self
            .conn
            .query(
                "SELECT d.jurisdiction_type, count(DISTINCT d.id), \
                        count(DISTINCT l.decision_id), count(l.id), \
                        count(l.target_decision_id) \
                 FROM decisions d LEFT JOIN decision_links l ON l.decision_id = d.id \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_type = ANY($1) \
                 GROUP BY 1 ORDER BY 1",
                &[&jurisdiction_types],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
            .collect())
    }

    /// Whether one of `dockets` touches a pending row of `table`, through the
    /// partial GIN index on `lj_docket_keys(ARRAY[target_docket])`.
    ///
    /// A count and not a `LIMIT 1`: the limit makes the planner walk the whole
    /// pending set hoping for an early hit that almost never comes. The keys go
    /// in as a subquery ([`BATCH_KEYS`]).
    async fn has_pending_docket(&self, table: &str, dockets: &[String]) -> Result<bool> {
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT count(*) FROM {table} \
                     WHERE target_decision_id IS NULL AND target_docket IS NOT NULL \
                       AND lj_docket_keys(ARRAY[target_docket]) && {BATCH_KEYS}"
                ),
                &[&dockets],
            )
            .await?;
        Ok(row.get::<_, i64>(0) > 0)
    }
}

/// Scope of a resolution pass: every pending row (the `reconcile` sweep), the
/// pending rows of the CITING decisions of a batch (`$1` = ids), or the ones
/// the dockets of a batch of TARGET decisions touch (`$1` = dockets, reverse
/// probe when the target arrives).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ResolveScope {
    All,
    ByDecision,
    ByArrival,
}

/// The UPDATEs that resolve a pending address of `table`, a pure function
/// (dumpable for psql validation). `scope` bounds the pending set to a batch
/// (`$1`).
///
/// One shape for every court, where eight families used to differ, plus the two
/// whose graphies folding does not merge: the eras of a Tribunal des conflits
/// number, and the CJUE role whose document set holds the judgment beside the
/// opinion. An address without a docket is not here: it is the arrêt a pourvoi
/// attacks, and the common parties resolve it.
pub(super) fn resolve_case_address_sqls(table: &str, scope: ResolveScope) -> Vec<String> {
    let bounded = scope != ResolveScope::All;
    let scope = match scope {
        ResolveScope::All => String::new(),
        ResolveScope::ByDecision => " AND p.decision_id = ANY($1)".to_string(),
        ResolveScope::ByArrival => {
            format!(" AND lj_docket_keys(ARRAY[p.target_docket]) && {BATCH_KEYS}")
        }
    };
    let fence = if bounded { " MATERIALIZED" } else { "" };
    let head = resolve_head(table, &scope, fence);
    let unique = resolve_unique(table, bounded);
    vec![
        format!(
            "{head} \
             SELECT p.target_court, p.target_docket, p.target_date, {CITED_KEY} AS dockets \
             FROM pending p \
             WHERE p.target_court IS DISTINCT FROM 'tc' \
               AND p.target_court IS DISTINCT FROM 'cjue' \
             {unique}"
        ),
        // TC: the stored number changes with the era (`00012` before 2000,
        // `3220` until 2004, `C4174` after) and practitioners write one for
        // another, so the probe carries the three graphies. Canonisation does
        // not merge them, a leading zero and a `c` are not punctuation.
        format!(
            "{head} \
             SELECT p.target_court, p.target_docket, p.target_date, \
                    lj_docket_keys(ARRAY[p.target_docket, \
                                         lpad(p.target_docket, 5, '0'), \
                                         'c' || p.target_docket]) AS dockets \
             FROM pending p WHERE p.target_court = 'tc' \
             {unique}"
        ),
        // CJUE: the docket names the CASE, which the judgment, the opinion and
        // the orders share, so a non-unique bearer snaps to the judgment (CELEX
        // `6YYYY<court>J…` in source_uid). A bare docket (`aff. 6/64`) is a `C-`.
        format!(
            "{head} \
             SELECT p.target_court, p.target_docket, p.target_date, \
                    lj_docket_keys(ARRAY[ \
                      CASE WHEN p.target_docket LIKE '_-%' THEN p.target_docket \
                           ELSE 'C-' || p.target_docket END]) AS dockets, \
                    CASE WHEN p.target_docket LIKE '_-%' \
                         THEN upper(left(p.target_docket, 1)) \
                         ELSE 'C' END AS court \
             FROM pending p WHERE p.target_court = 'cjue' \
             ), resolved AS{fence} ( \
               SELECT r.target_court, r.target_docket, r.target_date, \
                      (SELECT CASE \
                                WHEN count(*) = 1 THEN min(c.id) \
                                WHEN count(*) FILTER (WHERE c.is_arret) = 1 \
                                     THEN min(c.id) FILTER (WHERE c.is_arret) \
                              END \
                       FROM (SELECT t.decision_id AS id, EXISTS ( \
                                 SELECT 1 FROM decision_sources s \
                                 WHERE s.decision_id = t.decision_id \
                                   AND s.source_uid LIKE 'cjue/6____' || r.court || 'J%') AS is_arret \
                             FROM decision_text t \
                             WHERE t.deleted_at IS NULL AND t.counted AND t.kind = 'decision' \
                               AND t.id @@@ paradedb.term_set('docket_keys', r.dockets) \
                               AND t.jurisdiction_type = 'CJUE') c \
                      ) AS tid \
               FROM keys r){tail}",
            tail = resolve_tail(table, bounded),
        ),
    ]
}

/// Head shared by the resolution UPDATEs: the pending set, deduplicated by
/// address, so one address resolves once whatever its number of occurrences. An
/// address with no docket cannot drive the probe and never enters.
///
/// A bounded pass materialises `pending` and `resolved`: inlined, the planner
/// reads the whole pending set through `pending_addr_idx` for a batch that
/// touches a few rows.
fn resolve_head(table: &str, scope: &str, fence: &str) -> String {
    format!(
        "WITH pending AS{fence} ( \
           SELECT DISTINCT p.target_court, p.target_docket, p.target_date FROM {table} p \
           WHERE p.target_decision_id IS NULL AND p.target_docket IS NOT NULL{scope} \
         ), keys AS ("
    )
}

/// Tail shared by them: poses `target_decision_id` on every pending occurrence
/// of the resolved address.
///
/// A bounded pass also reaches those occurrences through the GIN, by the keys
/// of the resolved addresses gathered once: `IS NOT DISTINCT FROM` serves no
/// index, and the join would read every pending row of `case_citation`. An
/// occurrence carries the docket of its address, hence its
/// keys, so the predicate drops none.
fn resolve_tail(table: &str, bounded: bool) -> String {
    let by_keys = if bounded {
        " AND c.target_docket IS NOT NULL \
          AND lj_docket_keys(ARRAY[c.target_docket]) && ( \
            SELECT array_agg(k) FROM resolved r, \
                   unnest(lj_docket_keys(ARRAY[r.target_docket])) k \
            WHERE r.tid IS NOT NULL)"
    } else {
        ""
    };
    format!(
        " \
         UPDATE {table} c \
         SET target_decision_id = m.tid \
         FROM resolved m \
         WHERE c.target_court IS NOT DISTINCT FROM m.target_court \
           AND c.target_docket IS NOT DISTINCT FROM m.target_docket \
           AND c.target_date IS NOT DISTINCT FROM m.target_date \
           AND c.target_decision_id IS NULL \
           AND m.tid IS NOT NULL{by_keys}"
    )
}

/// The keys of an arriving batch, behind a subquery the planner cannot read: on
/// the literal array it scales its estimate with the batch, far past the real
/// rows, and trades the GIN for a seq scan.
const BATCH_KEYS: &str = "(SELECT lj_docket_keys($1))";

/// The cited number, canonised. `lj_docket_keys` is the function the
/// `docket_keys` field of `decisions_bm25` is built from, so probe and index
/// cannot drift, and neither can drift from `lj_core::docket_key`.
const CITED_KEY: &str = "lj_docket_keys(ARRAY[p.target_docket])";

/// The docket drives, the court and the date filter, and an absent component
/// drops its conjunct instead of making the address inert. Nothing is posed
/// until a single bearer answers: a canonisation collision returns two bearers,
/// so a refusal, never a wrong target. An address without a date never resolves
/// to a consultative opinion of the Conseil d'État, whose number an older judgment may carry.
///
/// `OFFSET 0` fences the docket probe from the other two: joined, the planner
/// drives on the jurisdiction and reads a whole court per address.
fn resolve_unique(table: &str, bounded: bool) -> String {
    let fence = if bounded { " MATERIALIZED" } else { "" };
    format!(
        "), resolved AS{fence} ( \
       SELECT r.target_court, r.target_docket, r.target_date, \
              (SELECT min(x.id) FROM \
                 (SELECT d.id, d.jurisdiction_code, d.date_lecture, d.seat_paths FROM decisions d \
                  WHERE d.deleted_at IS NULL AND d.kind = 'decision' \
                    AND lj_docket_keys(d.docket_numbers) && r.dockets \
                  OFFSET 0) x \
               WHERE (r.target_court IS NULL OR x.jurisdiction_code = r.target_court) \
                 AND (r.target_date IS NULL OR x.date_lecture = r.target_date) \
                 AND (r.target_date IS NOT NULL \
                      OR NOT coalesce('FR.CE.AVIS' = ANY(x.seat_paths), false)) \
               HAVING count(*) = 1) AS tid \
       FROM keys r)"
    ) + &resolve_tail(table, bounded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p lj-store resolve_case_address -- --nocapture` dumpe les
    /// UPDATE pour validation psql.
    #[test]
    fn resolve_case_address_sqls_cover_the_courts_and_the_scope() {
        let all = resolve_case_address_sqls("case_citation", ResolveScope::All);
        assert_eq!(all.len(), 3);
        for s in &all {
            assert!(s.contains("UPDATE case_citation"), "pas un UPDATE : {s}");
            assert!(!s.contains("$1"), "portée $1 en mode global : {s}");
            println!("{s};\n");
        }
        for s in all.iter().filter(|s| !s.contains("'cjue'")) {
            assert!(
                s.contains("lj_docket_keys(d.docket_numbers) && r.dockets")
                    && s.contains("OFFSET 0"),
                "sonde docket sans barrière : {s}"
            );
        }
        let by_decision = resolve_case_address_sqls("case_citation", ResolveScope::ByDecision);
        assert!(by_decision
            .iter()
            .all(|s| s.contains("p.decision_id = ANY($1)")));
        let arrival = resolve_case_address_sqls("decision_links", ResolveScope::ByArrival);
        assert!(arrival.iter().all(|s| s.contains("UPDATE decision_links")
            && s.contains(
                "lj_docket_keys(ARRAY[p.target_docket]) && (SELECT lj_docket_keys($1))"
            )));
        for s in by_decision.iter().chain(&arrival) {
            assert!(
                s.contains("pending AS MATERIALIZED")
                    && s.contains("resolved AS MATERIALIZED")
                    && s.contains("lj_docket_keys(ARRAY[c.target_docket]) &&"),
                "passe bornee sans barriere ni cles cote cible : {s}"
            );
            println!("{s};\n");
        }
        assert!(all
            .iter()
            .all(|s| !s.contains("MATERIALIZED") && !s.contains("c.target_docket IS NOT NULL")));
    }
}
