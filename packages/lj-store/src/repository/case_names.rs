//! Noms d'usage et leur écriture : résolution de l'ancre {juridiction, date,
//! numéro} contre le fonds, puis pose sous le barème `lj_case_name_rank`. Tout
//! écrivain passe par là, la projection des renvois comme un catalogue.

use chrono::{Datelike, NaiveDate};

use super::{DecisionRepository, ElectedCaseName};
use crate::error::Result;

/// The registry fields as each source writes them: Judilibre keys `chamber` and
/// `formation` at the top of its record, the opendata export
/// `Audience.Formation_Jugement`, the JADE export `META_JURI_ADMIN.FORMATION`,
/// the other sources none.
const REGISTRY_FIELDS_SQL: &str = "\
    CASE ds.source WHEN 'judilibre' THEN ds.source_fields->>'chamber' END AS chamber, \
    CASE ds.source \
         WHEN 'judilibre' THEN ds.source_fields->>'formation' \
         WHEN 'opendata' THEN ds.source_fields->'Audience'->>'Formation_Jugement' \
         WHEN 'dila-jade' THEN ds.source_fields->'META_JURI_ADMIN'->>'FORMATION' \
    END AS formation";

/// The authority source of a decision, the one the extraction reads.
const AUTHORITY_ORDER_SQL: &str =
    "ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, (ds.lang = 'eng') IS TRUE DESC, ds.source_uid ASC";

/// A decision read on a given day: its title and the opening of its text.
#[derive(Debug, Clone)]
pub struct DayHead {
    pub decision_id: i64,
    pub jurisdiction: String,
    pub date: NaiveDate,
    pub head: String,
}

/// One row of the seat vocabulary: a seat as written, the registry keys it came
/// with, and the months the corpus spans under them.
#[derive(Debug, Clone)]
pub struct SeatSample {
    pub seat: lj_core::chamber::Seat,
    pub chamber: Option<String>,
    pub formation: Option<String>,
    pub span: Option<(i32, i32)>,
}

/// A named decision as the body-citation detector reads it.
#[derive(Debug, Clone)]
pub struct NamedCaseRow {
    pub keys: Vec<String>,
    pub jurisdiction_code: Option<String>,
    pub docket_numbers: Vec<String>,
    pub date_lecture: Option<String>,
    pub citations: i64,
}

impl DecisionRepository<'_> {
    /// Résout des ancres contre le fonds, dans l'ordre des entrées. Les zéros
    /// de tête ne comptent pas : la source écrit `1645` pour Dehaene, que le
    /// fonds porte en `01645`.
    pub async fn resolve_docket_anchors(
        &self,
        jurisdictions: &[&str],
        dates: &[NaiveDate],
        dockets: &[&str],
    ) -> Result<Vec<Option<i64>>> {
        let rows = self
            .conn
            .query(
                "SELECT t.n, (SELECT min(d.id) FROM decisions d
                              WHERE d.jurisdiction_code = t.jur
                                AND d.date_lecture = t.lue
                                AND d.deleted_at IS NULL
                                AND EXISTS (
                                    SELECT 1
                                    FROM unnest(lj_docket_keys(d.docket_numbers)) k
                                    WHERE ltrim(k, '0') = ltrim(lower(t.num), '0')))
                 FROM unnest($1::text[], $2::date[], $3::text[])
                      WITH ORDINALITY AS t(jur, lue, num, n)
                 ORDER BY t.n",
                &[&jurisdictions, &dates, &dockets],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(1)).collect())
    }

    /// Each decision's public id, title and first 4,000 characters of text.
    pub async fn decision_heads(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, coalesce(d.public_id, ''),
                        coalesce(d.search_title, '') || ' ' || coalesce(
                            (SELECT left(t.full_text, 4000) FROM decision_text t
                             WHERE t.decision_id = d.id ORDER BY t.id LIMIT 1), '')
                 FROM decisions d WHERE d.id = ANY($1)",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// The HAL deposits already read, whatever they gave.
    pub async fn doctrine_visited(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query("SELECT hal_id FROM doctrine_deposits", &[])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Records a deposit read, `None` when its file could not be read.
    pub async fn put_doctrine_deposit(
        &self,
        hal_id: &str,
        citations: Option<&serde_json::Value>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO doctrine_deposits (hal_id, citations) VALUES ($1, $2)
                 ON CONFLICT (hal_id) DO UPDATE
                 SET visited_at = now(), citations = EXCLUDED.citations",
                &[&hal_id, &citations],
            )
            .await?;
        Ok(())
    }

    /// Every deposit that gave a name, with what it gave.
    pub async fn doctrine_citations(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT hal_id, citations FROM doctrine_deposits
                 WHERE jsonb_array_length(citations) > 0",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The name each decision carries, and its writer.
    pub async fn current_case_names(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, Option<String>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, case_name, case_name_source FROM decisions WHERE id = ANY($1)",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// The live decisions under these ECLIs, as `(ecli, decision)`.
    pub async fn resolve_eclis(&self, eclis: &[String]) -> Result<Vec<(String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT ecli, id FROM decisions WHERE ecli = ANY($1) AND deleted_at IS NULL",
                &[&eclis],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The decisions each court read on each day, with their title and the first
    /// 4,000 characters of their text.
    pub async fn day_heads(
        &self,
        jurisdictions: &[&str],
        dates: &[NaiveDate],
    ) -> Result<Vec<DayHead>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.jurisdiction_code, d.date_lecture,
                        coalesce(d.search_title, '') || ' ' || coalesce(
                            (SELECT left(t.full_text, 4000) FROM decision_text t
                             WHERE t.decision_id = d.id ORDER BY t.id LIMIT 1), '')
                 FROM unnest($1::text[], $2::date[]) AS k(jur, lue)
                 JOIN decisions d ON d.jurisdiction_code = k.jur AND d.date_lecture = k.lue
                 WHERE d.deleted_at IS NULL",
                &[&jurisdictions, &dates],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| DayHead {
                decision_id: r.get(0),
                jurisdiction: r.get(1),
                date: r.get(2),
                head: r.get(3),
            })
            .collect())
    }

    /// Poses each decision's Wikidata item and clears it from the decisions left
    /// out. Returns how many rows changed.
    pub async fn put_wikidata_ids(&self, items: &[(i64, String)]) -> Result<u64> {
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let qids: Vec<&str> = items.iter().map(|(_, q)| q.as_str()).collect();
        let set = self
            .conn
            .execute(
                "UPDATE decisions d SET wikidata_id = t.q
                 FROM unnest($1::int8[], $2::text[]) AS t(id, q)
                 WHERE d.id = t.id AND d.wikidata_id IS DISTINCT FROM t.q",
                &[&ids, &qids],
            )
            .await?;
        let cleared = self
            .conn
            .execute(
                "UPDATE decisions SET wikidata_id = NULL
                 WHERE wikidata_id IS NOT NULL AND NOT (id = ANY($1))",
                &[&ids],
            )
            .await?;
        Ok(set + cleared)
    }

    /// Pose des noms d'usage sans rien effacer : la forme affichée ne bouge que
    /// si le rang de la source atteint celui de l'écrivain en place, et les
    /// clés s'additionnent toujours. Une entrée sans nom n'apporte que ses
    /// clés. Rejouer n'écrit rien.
    ///
    /// Les clés d'une décision voyagent en une chaîne séparée par des virgules,
    /// que `case_name_key` ne peut produire : un tableau à deux dimensions
    /// exigerait qu'elles soient toutes en même nombre.
    pub async fn put_case_names(&self, elected: &[ElectedCaseName]) -> Result<u64> {
        let ids: Vec<i64> = elected.iter().map(|e| e.decision_id).collect();
        let names: Vec<Option<&str>> = elected.iter().map(|e| e.name.as_deref()).collect();
        let keys: Vec<String> = elected.iter().map(|e| e.keys.join(",")).collect();
        let sources: Vec<&str> = elected.iter().map(|e| e.source).collect();
        let poses = self
            .conn
            .execute(
                "WITH cible AS (
                     SELECT d.id, e.nom, e.src,
                            e.nom IS NOT NULL
                                AND lj_case_name_rank(d.case_name_source)
                                    <= lj_case_name_rank(e.src) AS gagne,
                            (SELECT array_agg(DISTINCT k ORDER BY k)
                             FROM unnest(coalesce(d.case_name_keys, '{}')
                                         || coalesce(string_to_array(nullif(e.cles, ''), ','),
                                                     '{}')) k) AS cles,
                            d.case_name AS nom_pose,
                            d.case_name_source AS src_pose,
                            d.case_name_keys AS cles_posees
                     FROM decisions d
                     JOIN unnest($1::int8[], $2::text[], $3::text[], $4::text[])
                          AS e(id, nom, cles, src) ON d.id = e.id
                 )
                 UPDATE decisions d SET
                     case_name = CASE WHEN c.gagne THEN c.nom ELSE c.nom_pose END,
                     case_name_source = CASE WHEN c.gagne THEN c.src ELSE c.src_pose END,
                     case_name_keys = c.cles
                 FROM cible c
                 WHERE d.id = c.id
                   AND (c.cles_posees IS DISTINCT FROM c.cles
                        OR (c.gagne AND (c.nom_pose IS DISTINCT FROM c.nom
                                         OR c.src_pose IS DISTINCT FROM c.src)))",
                &[&ids, &names, &keys, &sources],
            )
            .await?;
        Ok(poses)
    }

    /// The words of `words` too common to name a case alone: an index stopword, or a word
    /// written in `rarity` texts or more (« tacite », « paris », « martin »).
    pub async fn common_name_words(&self, words: &[&str], rarity: i64) -> Result<Vec<String>> {
        let (stop, rest): (Vec<&str>, Vec<&str>) = words
            .iter()
            .partition(|w| lj_textsearch::body_tok::is_stopword(w));
        let mut common: Vec<String> = stop.into_iter().map(str::to_string).collect();
        common.extend(self.frequent_names(&rest, rarity).await?);
        Ok(common)
    }

    /// Takes each `(decision, key)` pair out of the decision's keys, and its
    /// name with them when `source` wrote it and no key is left.
    pub async fn drop_case_name_keys(&self, source: &str, pairs: &[(i64, String)]) -> Result<u64> {
        let ids: Vec<i64> = pairs.iter().map(|(id, _)| *id).collect();
        let keys: Vec<&str> = pairs.iter().map(|(_, k)| k.as_str()).collect();
        let dropped = self
            .conn
            .execute(
                "WITH p AS (
                     SELECT id, array_agg(cle) AS cles
                     FROM unnest($1::int8[], $2::text[]) AS t(id, cle)
                     GROUP BY id
                 ), reste AS (
                     SELECT d.id,
                            nullif(array(SELECT k FROM unnest(d.case_name_keys) k
                                         WHERE k <> ALL(p.cles) ORDER BY k), '{}') AS cles
                     FROM decisions d JOIN p ON d.id = p.id
                     WHERE d.case_name_keys && p.cles
                 )
                 UPDATE decisions d SET
                     case_name_keys = r.cles,
                     case_name = CASE WHEN r.cles IS NULL AND d.case_name_source = $3
                                      THEN NULL ELSE d.case_name END,
                     case_name_source = CASE WHEN r.cles IS NULL AND d.case_name_source = $3
                                             THEN NULL ELSE d.case_name_source END
                 FROM reste r
                 WHERE d.id = r.id",
                &[&ids, &keys, &source],
            )
            .await?;
        Ok(dropped)
    }

    /// Every case name key, deduplicated. Exactly the domain of
    /// [`Self::case_name_match`], deletion filter included, so a query touching
    /// none of them names nothing.
    pub async fn case_name_key_set(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT k.cle
                 FROM decisions d, unnest(d.case_name_keys) AS k(cle)
                 WHERE d.case_name_keys IS NOT NULL AND d.deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Every named decision with its keys, read on their partial index.
    pub async fn named_decisions(&self) -> Result<Vec<(i64, Vec<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, case_name_keys FROM decisions
                 WHERE case_name_keys IS NOT NULL AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Among these keys, those at least `seuil` decisions use. What the fonds
    /// repeats is not a name: « Société » and « Ville de Paris » come out in far
    /// more decisions than « Czabaj » or « Société Tropic Travaux Signalisation ».
    /// A key of several words becomes a phrase with slop 2, the stop words
    /// holding a position in the index: `cour d appel` without slop misses most
    /// of its decisions. `case_name_key` lets through only alphanumerics and
    /// spaces, so quoting it cannot inject anything.
    ///
    /// The `LIMIT` makes the question binary, and that is what keeps it
    /// tractable: stopping at the threshold is far cheaper than counting. In
    /// batches of 60, under the pool's `statement_timeout`.
    pub async fn frequent_names(&self, cles: &[&str], seuil: i64) -> Result<Vec<String>> {
        let mut courants = Vec::new();
        for lot in cles.chunks(60) {
            let rows = self
                .conn
                .query(
                    "SELECT t.cle
                     FROM unnest($1::text[]) AS t(cle)
                     WHERE (SELECT count(*) FROM
                              (SELECT 1 FROM decision_text d
                               WHERE d.full_text @@@ CASE
                                       WHEN t.cle LIKE '% %' THEN '\"' || t.cle || '\"~2'
                                       ELSE t.cle END
                               LIMIT $2) x) >= $2",
                    &[&lot, &seuil],
                )
                .await?;
            courants.extend(rows.iter().map(|r| r.get::<_, String>(0)));
        }
        Ok(courants)
    }

    /// The texts writing a case name key followed by a roman numeral I to V,
    /// one phrase query per numeral on the BM25 index.
    pub async fn ordinal_citation_texts(&self, key: &str, romans: &[&str]) -> Result<Vec<String>> {
        let phrases: Vec<Vec<String>> = romans
            .iter()
            .map(|r| {
                key.split(' ')
                    .chain(std::iter::once(*r))
                    .map(str::to_string)
                    .collect()
            })
            .collect();
        let should: Vec<String> = (1..=phrases.len())
            .map(|i| format!("paradedb.phrase('full_text', ${i}::text[])"))
            .collect();
        let sql = format!(
            "SELECT d.full_text FROM decision_text d
             WHERE d.id @@@ paradedb.boolean(should => ARRAY[{}])
               AND d.deleted_at IS NULL
             LIMIT 300",
            should.join(", ")
        );
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = phrases
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let rows = self.conn.query(sql.as_str(), &params).await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// `(id, jurisdiction_code, docket keys, reading day ISO)` of decisions.
    pub async fn decision_anchors(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, Option<String>, Vec<String>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.jurisdiction_code, lj_docket_keys(d.docket_numbers),
                        to_char(d.date_lecture, 'YYYY-MM-DD')
                 FROM decisions d WHERE d.id = ANY($1)",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// The named decisions, their keys and the identity the caller draws the pending key
    /// from, with the citations aiming at each: spans of `case_citation` and renvois, the
    /// count the search card ranks a name's bearers by.
    ///
    /// A pass over every named decision and the citations aiming at it, past the pool's
    /// `statement_timeout` on a loaded base: the timeout is lifted for the transaction.
    pub async fn named_cases(&self) -> Result<Vec<NamedCaseRow>> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<tokio_postgres::Row>> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            Ok(self
                .conn
                .query(
                    "SELECT d.case_name_keys, d.jurisdiction_code, d.docket_numbers,
                            to_char(d.date_lecture, 'YYYY-MM-DD'),
                            (SELECT count(*) FROM case_citation cc WHERE cc.target_decision_id = d.id)
                            + (SELECT count(*) FROM decision_renvoi r WHERE r.target_decision_id = d.id)
                     FROM decisions d
                     WHERE d.case_name_keys IS NOT NULL AND d.deleted_at IS NULL",
                    &[],
                )
                .await?)
        }
        .await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        let rows = result?;
        Ok(rows
            .iter()
            .map(|r| NamedCaseRow {
                keys: r.get(0),
                jurisdiction_code: r.get(1),
                docket_numbers: r.get(2),
                date_lecture: r.get(3),
                citations: r.get(4),
            })
            .collect())
    }

    /// The corpus room vocabulary: the seat as WRITTEN, and the registry fields
    /// that produced it. One row per distinct seat and registry key of the
    /// authority source, the one `seat_rows_after` reads, so a decision with
    /// three sources speaks once; with it the first and last month the corpus
    /// wrote that row, as `year * 12 + month`.
    ///
    /// A full pass over decisions joined to their sources, past the pool's
    /// `statement_timeout`: the timeout is lifted for the transaction. ParadeDB's aggregate
    /// custom scan returns silently wrong counts on a BM25 table, so it is cut for the same
    /// transaction.
    pub async fn seat_vocabulary(&self) -> Result<Vec<SeatSample>> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<tokio_postgres::Row>> = async {
            self.conn
                .batch_execute(
                    "SET LOCAL statement_timeout = 0; \
                     SET LOCAL paradedb.enable_aggregate_custom_scan = off",
                )
                .await?;
            let sql = format!(
                "SELECT seat, chamber, formation, min(date_lecture), max(date_lecture) FROM (
                     SELECT d.seat, {REGISTRY_FIELDS_SQL}, d.date_lecture,
                            row_number() OVER (PARTITION BY ds.decision_id
                                               ORDER BY {AUTHORITY_ORDER_SQL}) AS rn
                     FROM decisions d
                     JOIN decision_sources ds ON ds.decision_id = d.id
                     WHERE d.seat IS NOT NULL AND d.deleted_at IS NULL
                 ) s WHERE rn = 1 GROUP BY 1, 2, 3"
            );
            Ok(self.conn.query(sql.as_str(), &[]).await?)
        }
        .await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        let rows = result?;
        let month = |d: NaiveDate| d.year() * 12 + d.month0() as i32;
        Ok(rows
            .iter()
            .map(|r| {
                let seat: tokio_postgres::types::Json<lj_core::chamber::Seat> = r.get(0);
                let first: Option<NaiveDate> = r.get(3);
                let last: Option<NaiveDate> = r.get(4);
                SeatSample {
                    seat: seat.0,
                    chamber: r.get(1),
                    formation: r.get(2),
                    span: first.zip(last).map(|(a, b)| (month(a), month(b))),
                }
            })
            .collect())
    }

    /// `(id, seat, registry chamber, registry formation, stored paths)`. The
    /// registry fields come from the authority source, the one the extraction
    /// reads. A row whose seat was written outside the extraction, the gold
    /// layer being the one, has never had a path computed and carries NULL
    /// where the rest carries an array. Same absence, so the read levels it.
    pub async fn seat_rows_after(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<
        Vec<(
            i64,
            lj_core::chamber::Seat,
            Option<String>,
            Option<String>,
            Vec<String>,
        )>,
    > {
        let sql = format!(
            "SELECT d.id, d.seat, s.chamber, s.formation, coalesce(d.seat_paths, '{{}}')
             FROM decisions d
             LEFT JOIN LATERAL (
                 SELECT {REGISTRY_FIELDS_SQL}
                 FROM decision_sources ds WHERE ds.decision_id = d.id
                 ORDER BY {AUTHORITY_ORDER_SQL}
                 LIMIT 1
             ) s ON true
             WHERE d.id > $1 AND d.seat IS NOT NULL AND d.deleted_at IS NULL
             ORDER BY d.id LIMIT $2"
        );
        let rows = self.conn.query(sql.as_str(), &[&after, &limit]).await?;
        Ok(rows
            .iter()
            .map(|r| {
                let seat: tokio_postgres::types::Json<lj_core::chamber::Seat> = r.get(1);
                (r.get(0), seat.0, r.get(2), r.get(3), r.get(4))
            })
            .collect())
    }

    /// `updated_at` stays: a path is a filter key, not page content, and
    /// touching it would push the row to IndexNow and the sitemap.
    pub async fn put_seat_paths(&self, items: &[(i64, Vec<String>)]) -> Result<u64> {
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let paths: Vec<String> = items.iter().map(|(_, p)| p.join(",")).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE decisions d
                 SET seat_paths = string_to_array(v.paths, ',')
                 FROM unnest($1::int8[], $2::text[]) AS v(id, paths)
                 WHERE d.id = v.id
                   AND d.seat_paths IS DISTINCT FROM string_to_array(v.paths, ',')",
                &[&ids, &paths],
            )
            .await?)
    }

    /// `(id, seat paths, formation, office, procedure, stored formation paths)`,
    /// the inputs of the « Formation » filter beside what it holds.
    #[allow(clippy::type_complexity)]
    pub async fn formation_rows_after(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<
        Vec<(
            i64,
            Vec<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Vec<String>,
        )>,
    > {
        let rows = self
            .conn
            .query(
                "SELECT id, coalesce(seat_paths, '{}'), formation_uid, office_uid, procedure_uid,
                        coalesce(formation_paths, '{}')
                 FROM decisions
                 WHERE id > $1 AND deleted_at IS NULL
                 ORDER BY id LIMIT $2",
                &[&after, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5)))
            .collect())
    }

    /// `updated_at` stays, as for [`Self::put_seat_paths`].
    pub async fn put_formation_paths(&self, items: &[(i64, Vec<String>)]) -> Result<u64> {
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let paths: Vec<String> = items.iter().map(|(_, p)| p.join(",")).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE decisions d
                 SET formation_paths = string_to_array(nullif(v.paths, ''), ',')
                 FROM unnest($1::int8[], $2::text[]) AS v(id, paths)
                 WHERE d.id = v.id
                   AND coalesce(d.formation_paths, '{}')
                       IS DISTINCT FROM coalesce(string_to_array(nullif(v.paths, ''), ','), '{}')",
                &[&ids, &paths],
            )
            .await?)
    }

    /// Efface le nom des décisions que l'écrivain ne nomme plus. Il n'efface
    /// que le sien : un nom qu'une source porte survit à la projection.
    ///
    /// Deux formes, une par écrivain. Celui qui reprojette tout à chaque passe
    /// efface par le complément de son lot ; celui qui écrit décision par
    /// décision efface les siennes, que la ré-extraction laisserait autrement
    /// sur un nom que leur source ne donne plus.
    pub async fn clear_case_names_except(
        &self,
        sources: &[&str],
        kept: &[i64],
    ) -> Result<Vec<i64>> {
        self.clear(
            sources,
            "NOT EXISTS (SELECT 1 FROM unnest($2::int8[]) k(id) WHERE k.id = d.id)",
            kept,
        )
        .await
    }

    pub async fn clear_case_names(&self, source: &str, ids: &[i64]) -> Result<Vec<i64>> {
        self.clear(&[source], "d.id = ANY($2)", ids).await
    }

    /// Rend les décisions effacées : leur titre porte encore le nom, et seul
    /// l'appelant sait le recomposer.
    async fn clear(&self, sources: &[&str], portee: &str, ids: &[i64]) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "UPDATE decisions d
                     SET case_name = NULL, case_name_keys = NULL, case_name_source = NULL
                     WHERE d.case_name_source = ANY($1) AND {portee}
                     RETURNING d.id"
                ),
                &[&sources, &ids],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }
}
