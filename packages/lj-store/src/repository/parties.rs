//! A decision's actors as one jsonb document (`decision_actors`): written with
//! the extraction, never below a manual or gold version, resolved against the
//! entity registry in place. SQL reads them through `decision_actor`, or through
//! `lj_actor_rows` once the GIN `lj_actor_terms` has narrowed the documents.
//!
//! The caller (lj-ingest) folds and computes `resolve_key`/`nature`/mentions in
//! Rust (`lj_extract::parties`): the SQL never folds.

use std::collections::{HashMap, HashSet};

use super::types::DecisionPartyRow;
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::{ToSql, Type};

/// Bulk write item: `(decision_id, actors)`. `None` = an extraction without the
/// actor layer, written as an empty set.
pub type DecisionActorWriteItem = (i64, Option<Vec<DecisionPartyRow>>);

/// An actor as an element of its decision's document, `e` left to the links.
fn actor_element(decision_id: i64, r: &DecisionPartyRow) -> Result<serde_json::Value> {
    let invalid = |what: &str, value: &str| {
        StoreError::Invalid(format!(
            "decision_actors (decision {decision_id}): {what} {value:?}"
        ))
    };
    let q = match r.quality.as_str() {
        "party" => "p",
        "law_firm" => "l",
        "counsel_name" => "c",
        "intervenor" => "i",
        other => return Err(invalid("quality", other)),
    };
    let mut el = serde_json::Map::new();
    el.insert("q".into(), q.into());
    if let Some(side) = r.side.as_deref() {
        let s = match side {
            "applicant" => "a",
            "defendant" => "d",
            other => return Err(invalid("side", other)),
        };
        el.insert("s".into(), s.into());
    }
    el.insert("v".into(), r.value.as_str().into());
    el.insert("k".into(), r.resolve_key.as_str().into());
    if let Some(nature) = r.nature.as_deref() {
        let n = match nature {
            "physique" => "ph",
            "morale_privee" => "pr",
            "morale_publique" => "pu",
            other => return Err(invalid("nature", other)),
        };
        el.insert("n".into(), n.into());
    }
    if let Some(b) = &r.barreau {
        el.insert("b".into(), b.as_str().into());
    }
    if let Some(role) = &r.role {
        el.insert("r".into(), role.as_str().into());
    }
    for (key, ord) in [("f", r.firm_ord), ("p", r.party_ord), ("o", r.sub_ord)] {
        if let Some(ord) = ord {
            el.insert(key.into(), ord.into());
        }
    }
    if !r.mentions.is_empty() {
        let (pairs, odd) = r.mentions.as_chunks::<2>();
        let disjoint = odd.is_empty()
            && pairs
                .iter()
                .try_fold(i32::MIN, |end, [s, e]| (end <= *s && s < e).then_some(*e))
                .is_some();
        if !disjoint {
            return Err(invalid("mentions", &format!("{:?}", r.mentions)));
        }
        el.insert("m".into(), r.mentions.clone().into());
    }
    Ok(serde_json::Value::Object(el))
}

/// A document without the given keys in any of its elements.
fn without(mut actors: serde_json::Value, keys: &[&str]) -> serde_json::Value {
    if let serde_json::Value::Array(els) = &mut actors {
        for el in els {
            if let serde_json::Value::Object(el) = el {
                for key in keys {
                    el.remove(*key);
                }
            }
        }
    }
    actors
}

/// Gives each new actor the link of an old one equal but for its mentions: what
/// resolution reads is unchanged, so is the link.
fn carry_links(old: &serde_json::Value, new: &mut serde_json::Value) {
    let bare = |el: &serde_json::Value| {
        let mut el = el.clone();
        if let Some(el) = el.as_object_mut() {
            el.remove("e");
            el.remove("m");
        }
        el
    };
    let (Some(old), Some(new)) = (old.as_array(), new.as_array_mut()) else {
        return;
    };
    let mut linked: Vec<(serde_json::Value, &serde_json::Value)> = old
        .iter()
        .filter_map(|el| Some((bare(el), el.get("e")?)))
        .collect();
    for el in new {
        let key = bare(el);
        if let Some(i) = linked.iter().position(|(b, _)| *b == key) {
            let (_, e) = linked.remove(i);
            if let Some(el) = el.as_object_mut() {
                el.insert("e".into(), e.clone());
            }
        }
    }
}

/// A decision's actors unnested: `x` the document, `a` an actor.
const ACTORS: &str = "decision_actors x CROSS JOIN LATERAL public.lj_actor_rows(x.actors) a";

/// The actor `a` may link to a legal person unless it is a party named by initials alone
/// (`l.`, `e t`), what pseudonymisation leaves of a person and a short company name matches,
/// or a natural person whose name carries an initial (`dominique a`).
const PARTY_NAMED: &str = "(a.quality <> 'party' \
     OR (a.resolve_key !~ '^[^[:alnum:]]*([[:alnum:]][^[:alnum:]]+)*[[:alnum:]][^[:alnum:]]*$' \
         AND NOT (a.nature IS NOT DISTINCT FROM 'physique' \
                  AND a.resolve_key ~ '(^|\\s)[[:alnum:]]\\.?(\\s|,|$)')))";

/// Writes into the documents the links of the CTE `hits` (`decision_id`, `ord`,
/// `resolve_key`, `uid`), a NULL `uid` unlinking. An actor whose key no longer
/// matches, its document rewritten since, is left alone. Returns the number of
/// actors written.
fn link_sql(ctes: &str) -> String {
    links_then(ctes, "SELECT coalesce(sum(n), 0)::bigint FROM upd")
}

/// [`link_sql`] returning the keys of `hits` instead.
fn link_sql_returning_keys(ctes: &str) -> String {
    links_then(ctes, "SELECT DISTINCT resolve_key FROM hits")
}

fn links_then(ctes: &str, select: &str) -> String {
    format!(
        "WITH {ctes}, links AS ( \
             SELECT decision_id, count(*) AS n, array_agg(ord) AS ords, \
                    array_agg(resolve_key) AS keys, array_agg(uid) AS uids \
             FROM hits GROUP BY decision_id \
         ), upd AS ( \
             UPDATE decision_actors d \
             SET actors = public.lj_actor_set_entities(d.actors, l.ords, l.keys, l.uids) \
             FROM links l WHERE d.decision_id = l.decision_id \
             RETURNING l.n \
         ) \
         {select}"
    )
}

impl DecisionRepository<'_> {
    /// Writes one decision's actors: sugar over [`Self::replace_decision_actors_bulk`].
    pub async fn replace_decision_actors(
        &self,
        decision_id: i64,
        parties: Option<&[DecisionPartyRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_decision_actors_bulk(&[(decision_id, parties.map(<[_]>::to_vec))])
            .await
    }

    /// Writes the actor documents of a batch. A decision or document above
    /// `EXTRACT_VERSION` (manual or gold) is never replaced; an actor equal but for
    /// its mentions keeps its link; an empty set deletes the document; a written
    /// one is resolved at once ([`Self::resolve_pending_parties_scoped`]). **The caller
    /// owns the transaction.** Returns the decisions whose actors changed beyond
    /// their mentions, for the caller to date their page.
    #[tracing::instrument(name = "db.replace_decision_actors_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_decision_actors_bulk(
        &self,
        items: &[DecisionActorWriteItem],
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
        let mut docs: Vec<(i64, Option<serde_json::Value>)> = Vec::with_capacity(items.len());
        for (decision_id, parties) in items.iter().filter(|(id, _)| !protected.contains(id)) {
            let parties = parties.as_deref().unwrap_or(&[]);
            let doc = (!parties.is_empty())
                .then(|| {
                    parties
                        .iter()
                        .map(|r| actor_element(*decision_id, r))
                        .collect::<Result<Vec<_>>>()
                        .map(serde_json::Value::Array)
                })
                .transpose()?;
            docs.push((*decision_id, doc));
        }
        let mut current: HashMap<i64, Option<serde_json::Value>> = HashMap::new();
        for row in self
            .conn
            .query(
                "SELECT decision_id, extract_version, actors FROM decision_actors \
                 WHERE decision_id = ANY($1)",
                &[&ids],
            )
            .await?
        {
            let version: i16 = row.get(1);
            let actors: serde_json::Value = row.get(2);
            current.insert(
                row.get(0),
                (version <= lj_versions::EXTRACT_VERSION).then_some(actors),
            );
        }
        let mut visible: Vec<i64> = Vec::new();
        docs.retain_mut(|(id, doc)| {
            let old = match current.get(id) {
                None => None,
                Some(None) => return false,
                Some(Some(old)) => Some(old),
            };
            if let (Some(doc), Some(old)) = (doc.as_mut(), old) {
                carry_links(old, doc);
            }
            if doc.as_ref() == old {
                return false;
            }
            let shown = |d: Option<&serde_json::Value>| d.map(|d| without(d.clone(), &["e", "m"]));
            if shown(doc.as_ref()) != shown(old) {
                visible.push(*id);
            }
            true
        });
        if docs.is_empty() {
            return Ok(visible);
        }
        let changed: Vec<i64> = docs.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM decision_actors WHERE decision_id = ANY($1)",
                &[&changed],
            )
            .await?;
        let sink = self
            .conn
            .copy_in(
                "COPY decision_actors (decision_id, extract_version, actors) \
                 FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(sink, &[Type::INT8, Type::INT2, Type::JSONB]);
        tokio::pin!(writer);
        for (decision_id, doc) in &docs {
            if let Some(doc) = doc {
                writer
                    .as_mut()
                    .write(&[decision_id, &lj_versions::EXTRACT_VERSION, doc])
                    .await?;
            }
        }
        writer.finish().await?;
        if docs.iter().any(|(_, doc)| doc.is_some()) {
            self.resolve_pending_parties_scoped(&changed).await?;
        }
        Ok(visible)
    }

    async fn link(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<u64> {
        let n: i64 = self.conn.query_one(sql, params).await?.get(0);
        Ok(n as u64)
    }

    /// At-write resolution of a batch: the same scopes and the same uniqueness as
    /// [`Self::resolve_pending_parties`], bounded to the pending keys of the batch's
    /// decisions and probing the registry by index instead of unfolding it. The
    /// normalised-firm pass has no indexable form and stays with the full passes.
    /// **The caller owns the transaction.** Returns the number of actors linked.
    #[tracing::instrument(name = "db.resolve_pending_parties_scoped", skip_all, fields(db.system = "postgresql", ids = decision_ids.len()))]
    pub async fn resolve_pending_parties_scoped(&self, decision_ids: &[i64]) -> Result<u64> {
        let mut linked = 0u64;
        for sql in [
            morales_scoped_sql(),
            oacc_counsel_sql(PartyScope::ByDecision),
            oacc_firm_sql(PartyScope::ByDecision),
            cnb_fullname_sql(PartyScope::ByDecision),
            cnb_surname_sql(PartyScope::ByDecision),
        ] {
            linked += self.link(&sql, &[&decision_ids]).await?;
        }
        Ok(linked)
    }

    /// The normalised-firm pass alone, the share of a full `parties-relink`.
    pub async fn resolve_pending_cabinets(&self) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let r: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            self.conn.batch_execute(&cabinets_pick_sql()).await?;
            self.link(&cabinets_sql(), &[]).await
        }
        .await;
        match r {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Registry delta probe: the `cnb:` identities a reload adds or reactivates
    /// wake the pending counsel keys that named them, by folded denomination with
    /// its rotations or by compound surname, then both stages replay on those keys.
    pub async fn resolve_pending_counsel_for_added(&self, added_uids: &[String]) -> Result<u64> {
        if added_uids.is_empty() {
            return Ok(0);
        }
        let rows = self
            .conn
            .query(
                &format!(
                    "WITH cand AS ( \
                         SELECT v.k FROM entity e, \
                              LATERAL unnest(lj_fold_all(e.denominations)) AS f(name), \
                              LATERAL (VALUES (f.name), \
                                       (regexp_replace(f.name, '^(\\S+) (.*)$', '\\2 \\1')), \
                                       (regexp_replace(f.name, '^(.*) (\\S+)$', '\\2 \\1'))) AS v(k) \
                         WHERE e.uid = ANY($1) \
                         UNION \
                         SELECT unnest(ARRAY[e.surname_key, replace(e.surname_key, ' ', '-')]) \
                         FROM entity e WHERE e.uid = ANY($1) AND e.surname_key IS NOT NULL \
                     ) \
                     SELECT DISTINCT a.resolve_key \
                     FROM {ACTORS} JOIN cand c ON c.k = a.resolve_key \
                     WHERE public.lj_actor_terms(x.actors) && ARRAY(SELECT 'k|' || k FROM cand) \
                       AND a.entity_uid IS NULL AND a.quality = 'counsel_name'"
                ),
                &[&added_uids],
            )
            .await?;
        let keys: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        self.resolve_pending_counsel_by_keys(&keys).await
    }

    /// Replays both counsel stages on named keys: those an incoming identity woke,
    /// or those an unlink freed.
    pub async fn resolve_pending_counsel_by_keys(&self, keys: &[String]) -> Result<u64> {
        if keys.is_empty() {
            return Ok(0);
        }
        let linked = self
            .link(&cnb_fullname_sql(PartyScope::ByKey), &[&keys])
            .await?;
        Ok(linked
            + self
                .link(&cnb_surname_sql(PartyScope::ByKey), &[&keys])
                .await?)
    }

    /// Registry delta unlink: a new identity can break the uniqueness behind
    /// existing links, a surname now ambiguous at the decision's date or a full
    /// name with a namesake now active at that date. Those links fall back to
    /// pending, ready to resolve again. Only the neighbours of the added
    /// identities, by surname or denomination, are touched.
    pub async fn cnb_delta_unlink(&self, added_uids: &[String]) -> Result<u64> {
        if added_uids.is_empty() {
            return Ok(0);
        }
        let s = self
            .link(
                &link_sql(&format!(
                    "touched AS ( \
                         SELECT DISTINCT a.surname_key FROM entity a \
                         WHERE a.uid = ANY($1) AND a.surname_key IS NOT NULL \
                     ), siblings AS ( \
                         SELECT e.uid, e.surname_key FROM entity e \
                         JOIN touched t ON t.surname_key = e.surname_key \
                         WHERE e.uid LIKE 'cnb:%' \
                     ), hits AS ( \
                         SELECT x.decision_id, a.ord, a.resolve_key, NULL::text AS uid \
                         FROM {ACTORS} \
                         JOIN decisions d ON d.id = x.decision_id \
                         JOIN siblings s ON s.uid = a.entity_uid \
                         WHERE public.lj_actor_terms(x.actors) \
                               && ARRAY(SELECT 'e|' || uid FROM siblings) \
                           AND translate(a.resolve_key, '-', ' ') = s.surname_key \
                           AND (SELECT count(DISTINCT e2.uid) FROM entity e2 \
                                WHERE e2.uid LIKE 'cnb:%' \
                                  AND e2.surname_key = s.surname_key \
                                  AND lj_active_at(e2.activity, d.date_lecture)) > 1 \
                     )"
                )),
                &[&added_uids],
            )
            .await?;
        let n = self
            .link(
                &link_sql(&format!(
                    "names AS ( \
                         SELECT DISTINCT f.name FROM entity a, \
                              LATERAL unnest(lj_fold_all(a.denominations)) AS f(name) \
                         WHERE a.uid = ANY($1) \
                     ), siblings AS ( \
                         SELECT DISTINCT h.uid, n.name FROM names n \
                         CROSS JOIN LATERAL ( \
                             SELECT e.uid FROM entity e \
                             WHERE lj_fold_all(e.denominations) @> ARRAY[n.name] \
                               AND e.uid LIKE 'cnb:%' \
                               AND NOT (e.uid = ANY($1)) \
                             OFFSET 0) h \
                     ), hits AS ( \
                         SELECT DISTINCT x.decision_id, a.ord, a.resolve_key, NULL::text AS uid \
                         FROM {ACTORS} \
                         JOIN decisions d ON d.id = x.decision_id \
                         JOIN siblings s ON s.uid = a.entity_uid \
                         WHERE public.lj_actor_terms(x.actors) \
                               && ARRAY(SELECT 'e|' || uid FROM siblings) \
                           AND a.resolve_key IN ( \
                               s.name, \
                               regexp_replace(s.name, '^(\\S+) (.*)$', '\\2 \\1'), \
                               regexp_replace(s.name, '^(.*) (\\S+)$', '\\2 \\1')) \
                           AND NOT EXISTS ( \
                               SELECT 1 FROM ( \
                                   SELECT e2.uid, e2.barreau FROM entity e2 \
                                   WHERE lj_fold_all(e2.denominations) && ARRAY[ \
                                       a.resolve_key, \
                                       regexp_replace(a.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                                       regexp_replace(a.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')] \
                                     AND e2.uid LIKE 'cnb:%' \
                                     AND lj_active_at(e2.activity, d.date_lecture)) c \
                               HAVING (count(DISTINCT c.uid) = 1 \
                                       AND min(c.uid) = a.entity_uid) \
                                   OR (count(DISTINCT c.uid) FILTER ( \
                                           WHERE c.barreau IS NOT DISTINCT FROM a.barreau) = 1 \
                                       AND min(c.uid) FILTER ( \
                                           WHERE c.barreau IS NOT DISTINCT FROM a.barreau) \
                                           = a.entity_uid)) \
                     )"
                )),
                &[&added_uids],
            )
            .await?;
        Ok(s + n)
    }

    /// The full net of the same invariant: **every** `cnb:` link must be the one
    /// resolution would set today. [`Self::cnb_delta_unlink`] only sees the
    /// neighbourhood of the added identities; an ambiguity already in the stock
    /// escapes it. Both stages first narrow to the keys actually ambiguous. The
    /// full-name stage exceeds the pool's `statement_timeout`: the caller lifts
    /// it. Returns the keys freed.
    pub async fn cnb_unlink_ambiguous(&self) -> Result<Vec<String>> {
        let s = self
            .conn
            .query(
                &link_sql_returning_keys(&format!(
                    "ambigus AS ( \
                         SELECT surname_key AS key FROM entity \
                         WHERE uid LIKE 'cnb:%' AND surname_key IS NOT NULL \
                         GROUP BY 1 HAVING count(*) > 1 \
                     ), porteurs AS ( \
                         SELECT e.uid, e.surname_key AS key FROM entity e \
                         JOIN ambigus am ON am.key = e.surname_key \
                     ), hits AS ( \
                         SELECT x.decision_id, a.ord, a.resolve_key, NULL::text AS uid \
                         FROM {ACTORS} \
                         JOIN decisions d ON d.id = x.decision_id \
                         JOIN porteurs s ON s.uid = a.entity_uid \
                         WHERE public.lj_actor_terms(x.actors) \
                               && ARRAY(SELECT 'e|' || uid FROM porteurs) \
                           AND a.quality = 'counsel_name' \
                           AND translate(a.resolve_key, '-', ' ') = s.key \
                           AND (SELECT count(DISTINCT e2.uid) FROM entity e2 \
                                WHERE e2.uid LIKE 'cnb:%' \
                                  AND e2.surname_key = s.key \
                                  AND lj_active_at(e2.activity, d.date_lecture)) > 1 \
                     )"
                )),
                &[],
            )
            .await?;
        // Materialised CTEs: inlined, the planner probes the GIN again per triple.
        let n = self
            .conn
            .query(
                &link_sql_returning_keys(&format!(
                    "ambig AS MATERIALIZED ( \
                         SELECT f.name FROM entity e, \
                              LATERAL unnest(lj_fold_all(e.denominations)) f(name) \
                         WHERE e.uid LIKE 'cnb:%' \
                         GROUP BY 1 HAVING count(DISTINCT e.uid) > 1 \
                     ), porteurs AS MATERIALIZED ( \
                         SELECT DISTINCT h.uid, a.name FROM ambig a \
                         CROSS JOIN LATERAL ( \
                             SELECT e.uid FROM entity e \
                             WHERE lj_fold_all(e.denominations) @> ARRAY[a.name] \
                               AND e.uid LIKE 'cnb:%' \
                             OFFSET 0) h \
                     ), triples AS MATERIALIZED ( \
                         SELECT DISTINCT a.resolve_key, a.barreau, a.entity_uid, d.date_lecture \
                         FROM {ACTORS} \
                         JOIN decisions d ON d.id = x.decision_id \
                         JOIN porteurs s ON s.uid = a.entity_uid \
                         WHERE public.lj_actor_terms(x.actors) \
                               && ARRAY(SELECT DISTINCT 'e|' || uid FROM porteurs) \
                           AND a.quality = 'counsel_name' \
                           AND a.resolve_key IN (s.name, \
                               regexp_replace(s.name, '^(\\S+) (.*)$', '\\2 \\1'), \
                               regexp_replace(s.name, '^(.*) (\\S+)$', '\\2 \\1')) \
                     ), cand AS MATERIALIZED ( \
                         SELECT k.resolve_key, h.uid, h.ebarreau, h.activity \
                         FROM (SELECT DISTINCT resolve_key FROM triples) k \
                         CROSS JOIN LATERAL ( \
                             SELECT e.uid, e.barreau AS ebarreau, e.activity \
                             FROM entity e \
                             WHERE lj_fold_all(e.denominations) && ARRAY[ \
                                     k.resolve_key, \
                                     regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                                     regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')] \
                               AND e.uid LIKE 'cnb:%' \
                             OFFSET 0) h \
                     ), bad AS MATERIALIZED ( \
                         SELECT t.resolve_key, t.barreau, t.entity_uid, t.date_lecture \
                         FROM triples t \
                         JOIN cand c ON c.resolve_key = t.resolve_key \
                         WHERE lj_active_at(c.activity, t.date_lecture) \
                         GROUP BY 1, 2, 3, 4 \
                         HAVING NOT ((count(DISTINCT c.uid) = 1 \
                                      AND min(c.uid) = t.entity_uid) \
                                  OR (count(DISTINCT c.uid) FILTER ( \
                                          WHERE c.ebarreau IS NOT DISTINCT FROM t.barreau) = 1 \
                                      AND min(c.uid) FILTER ( \
                                          WHERE c.ebarreau IS NOT DISTINCT FROM t.barreau) \
                                          = t.entity_uid)) \
                     ), hits AS ( \
                         SELECT x.decision_id, a.ord, a.resolve_key, NULL::text AS uid \
                         FROM {ACTORS} \
                         JOIN decisions d ON d.id = x.decision_id \
                         JOIN bad b ON b.resolve_key = a.resolve_key \
                          AND b.barreau IS NOT DISTINCT FROM a.barreau \
                          AND b.entity_uid = a.entity_uid \
                          AND b.date_lecture IS NOT DISTINCT FROM d.date_lecture \
                         WHERE public.lj_actor_terms(x.actors) \
                               && ARRAY(SELECT DISTINCT 'e|' || entity_uid FROM bad) \
                           AND a.quality = 'counsel_name' \
                     )"
                )),
                &[],
            )
            .await?;
        let mut keys: Vec<String> = s.iter().chain(n.iter()).map(|r| r.get(0)).collect();
        keys.sort_unstable();
        keys.dedup();
        Ok(keys)
    }

    /// The full net of [`PARTY_NAMED`]: unlinks every party link it would no longer set.
    /// A scan of every document, past the pool's `statement_timeout`: the caller
    /// lifts it. Returns the number of actors unlinked.
    pub async fn party_unlink_unnamed(&self) -> Result<u64> {
        self.link(
            &link_sql(&format!(
                "hits AS ( \
                     SELECT x.decision_id, a.ord, a.resolve_key, NULL::text AS uid \
                     FROM {ACTORS} \
                     WHERE a.entity_uid IS NOT NULL \
                       AND a.quality = 'party' \
                       AND NOT {PARTY_NAMED} \
                 )"
            )),
            &[],
        )
        .await
    }

    /// Resolves the pending keys against `entity` by UNAMBIGUOUS folded
    /// denomination: a key matching several distinct entities stays pending.
    ///
    /// Two scopes: legal persons (`party`, `law_firm`) against `siren:`/`rna:`,
    /// completed for `law_firm` outside CC/CE by the normalised form restricted to
    /// the `cabinets` category; and counsel (`counsel_name`) against `cnb:` (full
    /// name with its token rotations, namesakes split by `entity.barreau`,
    /// uniqueness judged AT THE DATE among those `lj_active_at`, CC/CE excluded),
    /// with a sub-stage on the unique compound surname (`entity.surname_key`).
    ///
    /// CC/CE decisions alone resolve against `oacc:` (`counsel_name` to counsel,
    /// `law_firm` to `oacc:firm-%` firms by token subset), a unique hit required.
    /// `intervenor` is gated. Returns the number of actors linked.
    #[tracing::instrument(name = "db.resolve_pending_parties", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_parties(&self) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            // Selected into a table first: under an `UPDATE` Postgres parallelises
            // nothing.
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE lj_hit_morales ON COMMIT DROP AS \
                     WITH keys AS ( \
                         SELECT DISTINCT a.resolve_key FROM {ACTORS} \
                         WHERE a.entity_uid IS NULL \
                           AND a.quality IN ('party', 'law_firm') \
                     ), names AS ( \
                         SELECT e.uid, f.folded \
                         FROM entity e, \
                              LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                         WHERE e.uid NOT LIKE 'cnb:%' \
                           AND e.uid NOT LIKE 'oacc:%' \
                     ) \
                     SELECT n.folded AS k, min(n.uid) AS uid \
                     FROM names n \
                     JOIN keys k ON k.resolve_key = n.folded \
                     GROUP BY n.folded \
                     HAVING count(DISTINCT n.uid) = 1; \
                     ANALYZE lj_hit_morales"
                ))
                .await?;
            let n = self
                .link(
                    &resolve_sql(
                        None,
                        "lj_hit_morales",
                        "'party', 'law_firm'",
                        &format!("AND {PARTY_NAMED}"),
                    ),
                    &[],
                )
                .await?;
            self.conn.batch_execute(&cabinets_pick_sql()).await?;
            let g = self.link(&cabinets_sql(), &[]).await?;
            // The `cnb:` registry unfolded once for both counsel stages: at the full
            // pass's grain, hashing beats probing per key.
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE lj_reg_cnb ON COMMIT DROP AS {CNB_REG_SQL}; \
                     CREATE INDEX ON lj_reg_cnb (name); ANALYZE lj_reg_cnb; \
                     CREATE TEMP TABLE lj_pick_fullname ON COMMIT DROP AS {}; \
                     ANALYZE lj_pick_fullname",
                    cnb_fullname_pick_sql(PartyScope::All, Some("lj_reg_cnb"))
                ))
                .await?;
            let c = self
                .link(&cnb_fullname_update_sql(None, "lj_pick_fullname"), &[])
                .await?;
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE lj_pick_surname ON COMMIT DROP AS {}; \
                     ANALYZE lj_pick_surname",
                    cnb_surname_pick_sql(PartyScope::All, Some("lj_reg_cnb"))
                ))
                .await?;
            let s = self
                .link(&cnb_surname_update_sql(None, "lj_pick_surname"), &[])
                .await?;
            let o = self.link(&oacc_counsel_sql(PartyScope::All), &[]).await?;
            let f = self.link(&oacc_firm_sql(PartyScope::All), &[]).await?;
            Ok(n + g + c + s + o + f)
        }
        .await;
        match updated {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Link volumes: (actors, linked, `siren:`, `rna:`, `cnb:`, `oacc:`).
    pub async fn actor_stats(&self) -> Result<(i64, i64, i64, i64, i64, i64)> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*), count(entity_uid), \
                        count(*) FILTER (WHERE entity_uid LIKE 'siren:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'rna:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'cnb:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'oacc:%') \
                 FROM decision_actor",
                &[],
            )
            .await?;
        Ok((
            row.get(0),
            row.get(1),
            row.get(2),
            row.get(3),
            row.get(4),
            row.get(5),
        ))
    }
}

/// The scope of a pass: everything, a written batch (`decision_ids`), or the
/// pending keys a registry delta touched (`resolve_key`s).
#[derive(Clone, Copy)]
pub(super) enum PartyScope {
    All,
    ByDecision,
    ByKey,
}

impl PartyScope {
    /// The condition over the documents `x` and the actors `a`.
    fn fragment(self) -> &'static str {
        match self {
            PartyScope::All => "",
            PartyScope::ByDecision => "AND x.decision_id = ANY($1) ",
            PartyScope::ByKey => {
                "AND public.lj_actor_terms(x.actors) \
                     && ARRAY(SELECT 'k|' || k FROM unnest($1::text[]) k) \
                 AND a.resolve_key = ANY($1) "
            }
        }
    }
}

/// The unfolded `cnb:` registry: (`name`, `uid`, `ebarreau`, `activity`), one row
/// per folded denomination. `lj_fold_all(e.denominations) @> ARRAY[k]` is an
/// equality on the unfolded array, so unfolding once and hashing keeps the
/// semantics of the per-key probes.
pub(super) const CNB_REG_SQL: &str = "SELECT f.name, e.uid, e.barreau AS ebarreau, e.activity \
     FROM entity e, LATERAL unnest(lj_fold_all(e.denominations)) f(name) \
     WHERE e.uid LIKE 'cnb:%'";

/// How a full-name pick row matches its actor: same barreau, same date.
const FULLNAME_ON: &str = "AND r.barreau IS NOT DISTINCT FROM a.barreau \
     AND r.date_lecture IS NOT DISTINCT FROM d.date_lecture \
     AND d.jurisdiction_type NOT IN ('CC', 'CE')";

/// How a surname pick row matches its actor: same date.
const SURNAME_ON: &str = "AND r.date_lecture IS NOT DISTINCT FROM d.date_lecture \
     AND d.jurisdiction_type NOT IN ('CC', 'CE')";

/// Full name with its rotations to `cnb:`, **at the date**: uniqueness is judged
/// among the candidates `lj_active_at(activity, date_lecture)`, one group per
/// (key, barreau of the mention, decision date). The barreau tie-break reads
/// `entity.barreau`. One source for the three scopes.
pub(super) fn cnb_fullname_sql(scope: PartyScope) -> String {
    cnb_fullname_update_sql(Some(&cnb_fullname_pick_sql(scope, None)), "pick")
}

/// The full-name selection alone: (`k`, `barreau`, `date_lecture`, `uid`).
///
/// `reg` picks the matching strategy, which depends on the number of keys. `None`
/// probes `entity_folded_all_idx` per variant, right for the scoped passes and
/// their few hundred keys. `Some(rel)` joins the registry already unfolded: the
/// full pass is far past the point where hashing beats probing.
pub(super) fn cnb_fullname_pick_sql(scope: PartyScope, reg: Option<&str>) -> String {
    let cand = match reg {
        Some(rel) => format!(
            "SELECT v.resolve_key, v.barreau, v.date_lecture, r.uid AS entity_uid, r.ebarreau \
             FROM variant v \
             JOIN {rel} r ON r.name = v.k \
             WHERE lj_active_at(r.activity, v.date_lecture)"
        ),
        None => "SELECT v.resolve_key, v.barreau, v.date_lecture, h.uid AS entity_uid, h.ebarreau \
             FROM variant v \
             CROSS JOIN LATERAL ( \
                 SELECT e.uid, e.barreau AS ebarreau FROM entity e \
                 WHERE lj_fold_all(e.denominations) @> ARRAY[v.k] \
                   AND e.uid LIKE 'cnb:%' \
                   AND lj_active_at(e.activity, v.date_lecture) \
                 OFFSET 0) h"
            .to_string(),
    };
    format!(
        "WITH pend AS ( \
             SELECT DISTINCT a.resolve_key, a.barreau, d.date_lecture \
             FROM {ACTORS} \
             JOIN decisions d ON d.id = x.decision_id \
             WHERE a.entity_uid IS NULL \
               AND a.quality = 'counsel_name' \
               AND strpos(a.resolve_key, ' ') > 0 \
               AND d.jurisdiction_type NOT IN ('CC', 'CE') \
               {scope}\
         ), variant AS ( \
             SELECT resolve_key, barreau, date_lecture, resolve_key AS k FROM pend \
             UNION \
             SELECT resolve_key, barreau, date_lecture, \
                    regexp_replace(resolve_key, '^(\\S+) (.*)$', '\\2 \\1') FROM pend \
             UNION \
             SELECT resolve_key, barreau, date_lecture, \
                    regexp_replace(resolve_key, '^(.*) (\\S+)$', '\\2 \\1') FROM pend \
         ), cand AS ({cand} \
         ) \
         SELECT resolve_key AS k, barreau, date_lecture, \
                CASE \
                  WHEN count(DISTINCT entity_uid) = 1 THEN min(entity_uid) \
                  WHEN count(DISTINCT entity_uid) FILTER (WHERE ebarreau = barreau) = 1 \
                    THEN min(entity_uid) FILTER (WHERE ebarreau = barreau) \
                END AS uid \
         FROM cand GROUP BY resolve_key, barreau, date_lecture",
        scope = scope.fragment(),
        cand = cand
    )
}

/// Links the full-name stage from a selection `rel`, a CTE built of `pick` or a
/// table.
pub(super) fn cnb_fullname_update_sql(pick: Option<&str>, rel: &str) -> String {
    resolve_sql(pick, rel, "'counsel_name'", FULLNAME_ON)
}

/// Unique compound surname, **at the date**: the full-name guard stays undated
/// (a namesake full name, even inactive at the date, disqualifies the key), the
/// surname's uniqueness is judged among those active at the date.
///
/// The guard goes through `OFFSET 0`: without it the planner flattens the `NOT
/// EXISTS` into an anti-join, materialises the whole `cnb:` registry and computes
/// `lj_fold_all` per (key, counsel) instead of probing `entity_folded_all_idx`.
pub(super) fn cnb_surname_sql(scope: PartyScope) -> String {
    cnb_surname_update_sql(Some(&cnb_surname_pick_sql(scope, None)), "pick")
}

/// The surname selection alone: (`k`, `date_lecture`, `uid`). `reg` arbitrates
/// probing or hashing, as for [`cnb_fullname_pick_sql`].
pub(super) fn cnb_surname_pick_sql(scope: PartyScope, reg: Option<&str>) -> String {
    let eligible = match reg {
        Some(rel) => format!(
            "knames AS ( \
                 SELECT DISTINCT k.resolve_key, n.name \
                 FROM (SELECT DISTINCT resolve_key FROM pend) k, \
                      LATERAL unnest(ARRAY[ \
                          k.resolve_key, \
                          regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                          regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')]) n(name) \
             ), porte AS ( \
                 SELECT DISTINCT kn.resolve_key FROM knames kn JOIN {rel} r ON r.name = kn.name \
             ), eligible AS ( \
                 SELECT k.resolve_key FROM (SELECT DISTINCT resolve_key FROM pend) k \
                 LEFT JOIN porte m ON m.resolve_key = k.resolve_key \
                 WHERE m.resolve_key IS NULL \
             )"
        ),
        None => "eligible AS ( \
             SELECT DISTINCT k.resolve_key FROM pend k \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM ( \
                     SELECT 1 FROM entity e \
                     WHERE lj_fold_all(e.denominations) && ARRAY[ \
                             k.resolve_key, \
                             regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                             regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')] \
                       AND e.uid LIKE 'cnb:%' \
                     OFFSET 0) h) \
         )"
        .to_string(),
    };
    format!(
        "WITH pend AS ( \
             SELECT DISTINCT a.resolve_key, d.date_lecture \
             FROM {ACTORS} \
             JOIN decisions d ON d.id = x.decision_id \
             WHERE a.entity_uid IS NULL \
               AND a.quality = 'counsel_name' \
               AND (strpos(a.resolve_key, ' ') > 0 \
                    OR strpos(a.resolve_key, '-') > 0) \
               AND d.jurisdiction_type NOT IN ('CC', 'CE') \
               {scope}\
         ), {eligible} \
         SELECT p2.resolve_key AS k, p2.date_lecture, min(e.uid) AS uid \
         FROM pend p2 \
         JOIN eligible g ON g.resolve_key = p2.resolve_key \
         JOIN entity e ON e.surname_key = translate(p2.resolve_key, '-', ' ') \
         WHERE e.uid LIKE 'cnb:%' \
           AND lj_active_at(e.activity, p2.date_lecture) \
         GROUP BY p2.resolve_key, p2.date_lecture \
         HAVING count(DISTINCT e.uid) = 1",
        scope = scope.fragment(),
        eligible = eligible
    )
}

/// Links the surname stage from a selection `rel`, a CTE built of `pick` or a
/// table.
pub(super) fn cnb_surname_update_sql(pick: Option<&str>, rel: &str) -> String {
    resolve_sql(pick, rel, "'counsel_name'", SURNAME_ON)
}

/// Links the pending actors of `qualities` that the relation `r` (`k`, `uid`, …)
/// resolves, matched by `on` over the actor `a` and its decision `d`, the
/// documents reached through the GIN by `r.k`. `pick` is the query of a CTE
/// `rel`, `None` when `rel` is a table.
fn resolve_sql(pick: Option<&str>, rel: &str, qualities: &str, on: &str) -> String {
    let with = pick.map_or(String::new(), |q| format!("{rel} AS ({q}), "));
    link_sql(&format!(
        "{with}hits AS ( \
             SELECT x.decision_id, a.ord, a.resolve_key, r.uid \
             FROM {ACTORS} \
             JOIN decisions d ON d.id = x.decision_id \
             JOIN {rel} r ON r.k = a.resolve_key AND r.uid IS NOT NULL {on} \
             WHERE public.lj_actor_terms(x.actors) \
                   && ARRAY(SELECT DISTINCT 'k|' || k FROM {rel} WHERE uid IS NOT NULL) \
               AND a.entity_uid IS NULL \
               AND a.quality IN ({qualities}) \
         )"
    ))
}

/// Legal persons (`party`, `law_firm`) to `siren:`/`rna:` without ambiguity, the batch's
/// keys [`PARTY_NAMED`] lets link probed in the GIN of `entity`.
fn morales_scoped_sql() -> String {
    resolve_sql(
        Some(&format!(
            "WITH keys AS ( \
                 SELECT DISTINCT a.resolve_key FROM {ACTORS} \
                 WHERE x.decision_id = ANY($1) \
                   AND a.entity_uid IS NULL \
                   AND a.quality IN ('party', 'law_firm') \
                   AND {PARTY_NAMED} \
             ) \
             SELECT k.resolve_key AS k, min(h.uid) AS uid \
             FROM keys k \
             CROSS JOIN LATERAL ( \
                 SELECT e.uid FROM entity e \
                 WHERE lj_fold_all(e.denominations) @> ARRAY[k.resolve_key] \
                   AND e.uid NOT LIKE 'cnb:%' \
                   AND e.uid NOT LIKE 'oacc:%' \
                 OFFSET 0) h \
             GROUP BY k.resolve_key \
             HAVING count(DISTINCT h.uid) = 1"
        )),
        "pick",
        "'party', 'law_firm'",
        &format!("AND {PARTY_NAMED}"),
    )
}

/// Counsel to the Conseils, CC/CE decisions only: full name with its rotations,
/// or a lone name equal to a counsel's surname, a unique hit required. The
/// `oacc:` registry is tiny; probing its namespace needs
/// `entity_uid_pattern_idx`, or `LIKE 'oacc:%'` scans the whole of `entity`.
fn oacc_counsel_sql(scope: PartyScope) -> String {
    resolve_sql(
        Some(&format!(
            "WITH pend AS ( \
                 SELECT DISTINCT a.resolve_key AS k \
                 FROM {ACTORS} \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE a.entity_uid IS NULL \
                   AND a.quality = 'counsel_name' \
                   AND d.jurisdiction_type IN ('CC', 'CE') \
                   {scope}\
             ), variant AS ( \
                 SELECT k, k AS v FROM pend \
                 UNION \
                 SELECT k, regexp_replace(k, '^(\\S+) (.*)$', '\\2 \\1') \
                   FROM pend WHERE strpos(k, ' ') > 0 \
                 UNION \
                 SELECT k, regexp_replace(k, '^(.*) (\\S+)$', '\\2 \\1') \
                   FROM pend WHERE strpos(k, ' ') > 0 \
             ), av AS ( \
                 SELECT e.uid AS entity_uid, f.folded, \
                        split_part(f.folded, ' ', -1) AS surname \
                 FROM entity e, \
                      LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                 WHERE e.uid LIKE 'oacc:%' AND e.uid NOT LIKE 'oacc:firm-%' \
             ), cand AS ( \
                 SELECT v.k, a.entity_uid FROM variant v JOIN av a ON a.folded = v.v \
                 UNION \
                 SELECT p.k, a.entity_uid FROM pend p JOIN av a ON a.surname = p.k \
                   WHERE strpos(p.k, ' ') = 0 \
             ) \
             SELECT k, min(entity_uid) AS uid \
             FROM cand GROUP BY k HAVING count(DISTINCT entity_uid) = 1",
            scope = scope.fragment()
        )),
        "pick",
        "'counsel_name'",
        "AND d.jurisdiction_type IN ('CC', 'CE')",
    )
}

/// Firms of counsel to the Conseils, CC/CE decisions only: the folded key split
/// on non-alphanumerics, stripped of company forms, generic words and tokens
/// under 2 chars, links when all its tokens are in exactly one `oacc:firm-%`.
fn oacc_firm_sql(scope: PartyScope) -> String {
    resolve_sql(
        Some(&format!(
            "WITH pend AS ( \
                 SELECT DISTINCT a.resolve_key AS k \
                 FROM {ACTORS} \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE a.entity_uid IS NULL \
                   AND a.quality = 'law_firm' \
                   AND d.jurisdiction_type IN ('CC', 'CE') \
                   {scope}\
             ), ptok AS ( \
                 SELECT k, t.tok \
                 FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') AS t(tok) \
                 WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                     'scp','sarl','sarlu','sas','selas','selarl','selafa', \
                     'selasu','aarpi','societe','cabinet','avocat','avocats', \
                     'associe','associes','et','ou','de','du','des','la','le', \
                     'les','au','aux') \
                 GROUP BY k, t.tok \
             ), nkey AS ( \
                 SELECT k, count(*) AS nt FROM ptok GROUP BY k \
             ), ftok AS MATERIALIZED ( \
                 SELECT e.uid AS entity_uid, t.tok \
                 FROM entity e, \
                      LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded), \
                      LATERAL regexp_split_to_table(f.folded, '[^[:alnum:]]+') AS t(tok) \
                 WHERE e.uid LIKE 'oacc:firm-%' AND t.tok <> '' \
                 GROUP BY e.uid, t.tok \
             ), hit AS ( \
                 SELECT pt.k, f.entity_uid \
                 FROM ptok pt \
                 JOIN ftok f ON f.tok = pt.tok \
                 JOIN nkey nk ON nk.k = pt.k \
                 GROUP BY pt.k, f.entity_uid, nk.nt \
                 HAVING count(*) = nk.nt \
             ) \
             SELECT k, min(entity_uid) AS uid \
             FROM hit GROUP BY k HAVING count(DISTINCT entity_uid) = 1",
            scope = scope.fragment()
        )),
        "pick",
        "'law_firm'",
        "AND d.jurisdiction_type IN ('CC', 'CE')",
    )
}

/// Normalised firms: the `law_firm` keys still pending outside CC/CE compared in
/// normalised form (company forms, trade generics, conjunctions and tokens under
/// 2 chars dropped, order kept) against the denominations of the `cabinets`
/// category alone, a unique hit required, into `lj_pick_cabinets`. Keys holding
/// `[` are anonymisation placeholders. `lj_fold_all` can emit the same folded
/// twice: deduplicated before tokenising, or `string_agg` doubles the tokens.
/// The pending keys go through an analysed table: estimated at one row, they met
/// the firm names in a nested loop.
fn cabinets_pick_sql() -> String {
    format!(
        "CREATE TEMP TABLE lj_cab_keys ON COMMIT DROP AS \
         WITH pend AS ( \
             SELECT DISTINCT a.resolve_key AS k \
             FROM {ACTORS} \
             JOIN decisions d ON d.id = x.decision_id \
             WHERE a.entity_uid IS NULL \
               AND a.quality = 'law_firm' \
               AND d.jurisdiction_type NOT IN ('CC', 'CE') \
               AND strpos(a.resolve_key, '[') = 0 \
         ) \
         SELECT k, string_agg(tok, ' ' ORDER BY ord) AS norm \
         FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') \
              WITH ORDINALITY AS t(tok, ord) \
         WHERE length(t.tok) >= 2 AND t.tok NOT IN ({FIRM_WORDS}) \
         GROUP BY k; \
         ANALYZE lj_cab_keys; \
         CREATE TEMP TABLE lj_pick_cabinets ON COMMIT DROP AS \
         WITH dnorm AS MATERIALIZED ( \
             SELECT dd.entity_uid, string_agg(tok, ' ' ORDER BY ord) AS norm \
             FROM (SELECT DISTINCT e.uid AS entity_uid, f.folded \
                   FROM entity e, \
                        LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                   WHERE e.category = 'cabinets' AND e.uid LIKE 'siren:%') dd, \
                  LATERAL regexp_split_to_table(dd.folded, '[^[:alnum:]]+') \
                  WITH ORDINALITY AS t(tok, ord) \
             WHERE length(t.tok) >= 2 AND t.tok NOT IN ({FIRM_WORDS}) \
             GROUP BY dd.entity_uid, dd.folded \
         ) \
         SELECT h.k, min(h.entity_uid) AS uid \
         FROM (SELECT kn.k, dn.entity_uid \
               FROM lj_cab_keys kn JOIN dnorm dn ON dn.norm = kn.norm \
               GROUP BY kn.k, dn.entity_uid) h \
         GROUP BY h.k HAVING count(*) = 1; \
         ANALYZE lj_pick_cabinets"
    )
}

/// The normalised-firm pass over `lj_pick_cabinets`.
fn cabinets_sql() -> String {
    resolve_sql(
        None,
        "lj_pick_cabinets",
        "'law_firm'",
        "AND d.jurisdiction_type NOT IN ('CC', 'CE')",
    )
}

/// The words a normalised firm name drops.
const FIRM_WORDS: &str = "'scp','sarl','sarlu','sas','sasu','selarl','selarlu', \
     'selas','selasu','selafa','sel','aarpi','sep', \
     'societe','societes','cabinet','cabinets','etude', \
     'office','avocat','avocats','associe','associes', \
     'conseil','conseils','et','ou'";

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec: the cnb builders carry resolution at the date and the barreau
    /// tie-break by column, in the three scopes.
    #[test]
    fn cnb_sqls_cover_scopes_and_date_rules() {
        for (name, sql) in [
            ("fullname", cnb_fullname_sql(PartyScope::All)),
            ("surname", cnb_surname_sql(PartyScope::All)),
        ] {
            assert!(sql.contains("lj_active_at"), "{name}: unique at the date");
            assert!(sql.contains("d.date_lecture"), "{name}: date in the group");
            assert!(!sql.contains("$1"), "{name}: All takes no parameter");
            assert!(
                !sql.contains("split_part"),
                "{name}: no barreau from the uid"
            );
        }
        assert!(cnb_fullname_sql(PartyScope::All).contains("ebarreau = barreau"));
        assert!(cnb_fullname_sql(PartyScope::ByDecision).contains("AND x.decision_id = ANY($1)"));
        assert!(cnb_surname_sql(PartyScope::ByKey).contains("AND a.resolve_key = ANY($1)"));
        let surname = cnb_surname_sql(PartyScope::All);
        let eligible = surname
            .split("eligible AS")
            .nth(1)
            .unwrap()
            .split("SELECT p2.resolve_key")
            .next()
            .unwrap();
        assert!(!eligible.contains("lj_active_at"));
        assert!(cnb_surname_update_sql(None, "lj_pick_surname").contains("JOIN lj_pick_surname r"));
        assert!(!cnb_surname_pick_sql(PartyScope::All, None).contains("UPDATE"));
        assert!(!cnb_fullname_pick_sql(PartyScope::All, None).contains("UPDATE"));
        for scope in [PartyScope::All, PartyScope::ByKey, PartyScope::ByDecision] {
            let probe = cnb_fullname_pick_sql(scope, None);
            let hash = cnb_fullname_pick_sql(scope, Some("lj_reg_cnb"));
            assert!(probe.contains("lj_fold_all(e.denominations) @> ARRAY[v.k]"));
            assert!(!probe.contains("lj_reg_cnb"));
            assert!(hash.contains("JOIN lj_reg_cnb r ON r.name = v.k"));
            assert!(!hash.contains("lj_fold_all"));
            assert!(probe.contains("lj_active_at") && hash.contains("lj_active_at"));
        }
        let hash = cnb_surname_pick_sql(PartyScope::All, Some("lj_reg_cnb"));
        let guard = hash
            .split("porte AS")
            .nth(1)
            .unwrap()
            .split("SELECT p2.resolve_key")
            .next()
            .unwrap();
        assert!(!guard.contains("lj_active_at"));
    }
}
