//! Provenance, résolution d'identité et déduplication : sondes d'idempotence,
//! backfills, fusion de clusters doublons, retrait par provenance (tombstone) +
//! reconcile.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use lj_core::identity::{
    choose, text_containment, text_identity, Choice, Traits, TIED_TEXT_IDENTITY,
};

use super::decisions::canonical_jurisdiction_code;
use super::support::source_from_source_uid;
use super::types::{ExistingDecisionState, ExtractedFields, Identity, JurisdictionRow};
use super::DecisionRepository;
use crate::error::Result;
use serde_json::Value;

const DILA_COPY_CONTAINMENT: f64 = 0.8;
const DILA_DISTINCT_CONTAINMENT: f64 = 0.7;

/// The source that publishes decisions without their docket.
const LEGALIS: &str = "legalis";
/// Measured on 15 Legalis pages of courts of appeal: 0.96 to 0.99 against the decision each
/// copies, 0.07 at most against any other of the same court and day.
const LEGALIS_COPY_CONTAINMENT: f64 = 0.8;
const LEGALIS_DISTINCT_CONTAINMENT: f64 = 0.3;
/// Measured on the African Court's five pairs at one address: 1.0 for the two it posted twice,
/// 0.29 at most for the others.
pub const SOURCE_TWIN_CONTAINMENT: f64 = 0.8;

fn is_dila_judicial(source: &str) -> bool {
    matches!(source, "dila-cass" | "dila-inca" | "dila-capp")
}

pub struct ProvenanceToResolve {
    pub decision_id: i64,
    pub source_uid: String,
    pub source_fields: Value,
    pub read_text: String,
    /// Another active provenance sits on the same decision.
    pub shared: bool,
}

pub struct JoinderPair {
    pub copy: i64,
    pub dockets: Vec<String>,
    pub copy_text: String,
    pub keeper: i64,
    pub keeper_text: String,
}

/// One row of [`DecisionRepository::judilibre_provenances`].
pub struct JudilibreProvenance {
    pub source_uid: String,
    pub tombstoned: bool,
    pub jurisdiction: Option<String>,
    pub decision_date: Option<String>,
}

pub struct WebNoteHead {
    pub decision_id: i64,
    pub publisher: Option<String>,
    pub title: Option<String>,
}

/// The three judicial funds are one publisher: a decision holds one of their
/// records, the others are copies.
/// The other provenances of a Decisia item, whose French and English pages share its number: its
/// other language at A2AJ and, at the Supreme Court, the court's own page.
fn decisia_siblings(source_uid: &str) -> Option<Vec<String>> {
    let (dataset, item) = match source_uid.split('/').collect::<Vec<_>>()[..] {
        ["a2aj", dataset, item, _] => (dataset, item),
        ["scc-csc", item, _] => ("SCC", item),
        _ => return None,
    };
    let mut uids: Vec<String> = ["fr", "en"]
        .iter()
        .map(|l| format!("a2aj/{dataset}/{item}/{l}"))
        .collect();
    if dataset == "SCC" {
        uids.extend(["fr", "en"].map(|l| format!("scc-csc/{item}/{l}")));
    }
    uids.retain(|u| u != source_uid);
    Some(uids)
}

fn same_publisher(held: &str, incoming: &str) -> bool {
    held == incoming || (is_dila_judicial(held) && is_dila_judicial(incoming))
}

/// The languages held for a CELEX, each with the bytes of its text, and one's source fields.
pub type CjueHeld = (Vec<(String, i32)>, Value);

/// A live decision's id, court code and date.
pub type DecisionAddress = (i64, Option<String>, Option<NaiveDate>);

impl DecisionRepository<'_> {
    /// Sonde d'idempotence intra-source sur le pivot `decision_sources.source_uid`
    /// (UNIQUE). Renvoie `(decision_id, content_checksum, active)`, provenance
    /// tombstonée comprise : un `source_uid` connu mais tombstoné se ré-update en
    /// place. `None` = provenance jamais vue → résolution d'identité en aval.
    #[tracing::instrument(name = "db.find_provenance", skip(self), fields(db.system = "postgresql"))]
    pub async fn find_provenance(&self, source_uid: &str) -> Result<Option<(i64, String, bool)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT decision_id, content_checksum, deleted_at IS NULL AS active \
                 FROM decision_sources WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        Ok(row.map(|r| {
            (
                r.get::<_, i64>(0),
                r.get::<_, String>(1),
                r.get::<_, bool>(2),
            )
        }))
    }

    /// `source_uid` of the authority provenance, tombstoned included: highest
    /// rank, then French, then English, then smallest `source_uid`. `None` for a
    /// decision without provenance. Only the authority writes the metadata and
    /// the read text.
    #[tracing::instrument(name = "db.authority_source_uid", skip(self), fields(db.system = "postgresql"))]
    pub async fn authority_source_uid(&self, decision_id: i64) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT source_uid FROM decision_sources WHERE decision_id = $1 \
                 ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, (lang = 'eng') IS TRUE DESC, source_uid ASC LIMIT 1",
                &[&decision_id],
            )
            .await?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    }

    /// Toutes les provenances CNDA actives, avec de quoi recalculer `solution_uid`
    /// sans re-télécharger le PDF : `(source_uid, full_text, source_fields)`.
    #[tracing::instrument(name = "db.cnda_sources_for_solution", skip(self), fields(db.system = "postgresql"))]
    pub async fn cnda_sources_for_solution(&self) -> Result<Vec<(String, String, Value)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT s.source_uid, {text}, s.source_fields \
                 FROM decision_sources s JOIN decisions d ON d.id = s.decision_id \
                 WHERE s.source_uid LIKE 'cnda/%' \
                   AND s.deleted_at IS NULL \
                   AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)",
                    text = super::pick::text("p.full_text", "d")
                ),
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Aligne `solution_uid` dans les `source_fields` sur ce que le parser dit
    /// aujourd'hui : pose la clé pour `set`, la retire pour `remove`. Rejouable —
    /// chaque branche ne touche que les lignes qui changent. Renvoie
    /// `(posées, retirées)`.
    #[tracing::instrument(name = "db.align_source_fields_solution_uid", skip(self, set_uids, set_values, remove_uids), fields(db.system = "postgresql", set = set_uids.len(), remove = remove_uids.len()))]
    pub async fn align_source_fields_solution_uid(
        &self,
        set_uids: &[String],
        set_values: &[String],
        remove_uids: &[String],
    ) -> Result<(u64, u64)> {
        let posees = if set_uids.is_empty() {
            0
        } else {
            self.conn
                .execute(
                    "UPDATE decision_sources s \
                     SET source_fields = jsonb_set(s.source_fields, ARRAY['solution_uid'], \
                                                   to_jsonb(v.solution_uid)) \
                     FROM UNNEST($1::text[], $2::text[]) AS v(source_uid, solution_uid) \
                     WHERE s.source_uid = v.source_uid \
                       AND s.source_fields->>'solution_uid' IS DISTINCT FROM v.solution_uid",
                    &[&set_uids, &set_values],
                )
                .await?
        };
        let retirees = if remove_uids.is_empty() {
            0
        } else {
            self.conn
                .execute(
                    "UPDATE decision_sources s \
                     SET source_fields = s.source_fields - 'solution_uid' \
                     WHERE s.source_uid = ANY($1) \
                       AND s.source_fields->'solution_uid' IS NOT NULL",
                    &[&remove_uids],
                )
                .await?
        };
        Ok((posees, retirees))
    }

    /// Poses one `source_fields` key on these provenances, `values` carrying its
    /// JSON as the ingest writes it, over the corpus that predates it.
    #[tracing::instrument(name = "db.set_source_field", skip(self, source_uids, values), fields(db.system = "postgresql", n = source_uids.len()))]
    pub async fn set_source_field(
        &self,
        field: &str,
        source_uids: &[String],
        values: &[String],
    ) -> Result<u64> {
        if source_uids.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "UPDATE decision_sources s \
                 SET source_fields = jsonb_set(s.source_fields, ARRAY[$3::text], v.value::jsonb) \
                 FROM UNNEST($1::text[], $2::text[]) AS v(source_uid, value) \
                 WHERE s.source_uid = v.source_uid \
                   AND s.deleted_at IS NULL \
                   AND s.source_fields->$3::text IS DISTINCT FROM v.value::jsonb",
                &[&source_uids, &values, &field],
            )
            .await?)
    }

    #[tracing::instrument(name = "db.upsert_decision_source", skip(self, source_fields), fields(db.system = "postgresql"))]
    pub async fn upsert_decision_source(
        &self,
        decision_id: i64,
        source_uid: &str,
        content_checksum: &str,
        payload_format: &str,
        source_fields: &Value,
    ) -> Result<()> {
        // `source_rank` est une colonne **générée** depuis `source`, jamais écrite ici.
        let source = source_from_source_uid(source_uid);
        // `lang` = langue de la source, **dite par l'ingester** dans
        // `source_fields["lang"]` (CEDH/CJUE ; 'fra'/'eng'), matérialisée en colonne.
        // Absente pour les sources FR-only → NULL.
        let lang = source_fields.get("lang").and_then(|v| v.as_str());
        self.conn
            .execute(
                "
                INSERT INTO decision_sources
                  (decision_id, source, source_uid, content_checksum,
                   payload_format, source_fields, lang, deleted_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7,
                        CASE WHEN lj_decision_withdrawn($1) THEN NOW() END)
                ON CONFLICT (source_uid) DO UPDATE SET
                  decision_id = EXCLUDED.decision_id,
                  source = EXCLUDED.source,
                  content_checksum = EXCLUDED.content_checksum,
                  payload_format = EXCLUDED.payload_format,
                  source_fields = EXCLUDED.source_fields,
                  lang = EXCLUDED.lang,
                  deleted_at = CASE WHEN decision_sources.expired_checksum = EXCLUDED.content_checksum
                                    THEN coalesce(decision_sources.deleted_at, NOW())
                                    ELSE EXCLUDED.deleted_at END,
                  ingested_at = NOW()
                ",
                &[
                    &decision_id,
                    &source,
                    &source_uid,
                    &content_checksum,
                    &payload_format,
                    &source_fields,
                    &lang,
                ],
            )
            .await?;
        Ok(())
    }

    /// Backfill batché (keyset par `decisions.id`) de `decisions.ecli` depuis
    /// `decision_sources.source_fields->>'ecli'` de la provenance autoritaire.
    /// Idempotent : n'écrit que les lignes dont `ecli IS NULL` et dont `source_fields`
    /// porte la clé. L'ECLI n'étant pas unique, il peut se poser sur plusieurs
    /// décisions distinctes. Renvoie `(rows_updated, last_id)`, `None` quand le lot est
    /// vide.
    #[tracing::instrument(name = "db.backfill_ecli_batch", skip(self), fields(db.system = "postgresql"))]
    pub async fn backfill_ecli_batch(
        &self,
        after_id: i64,
        batch: i64,
    ) -> Result<Option<(u64, i64)>> {
        let row = self
            .conn
            .query_one(
                "
                WITH lot AS (
                    SELECT d.id
                    FROM decisions d
                    WHERE d.id > $1
                    ORDER BY d.id
                    LIMIT $2
                ),
                upd AS (
                    UPDATE decisions d
                    SET ecli = ds.source_fields->>'ecli'
                    FROM lot l
                    JOIN LATERAL (
                        SELECT s.source_fields
                        FROM decision_sources s
                        WHERE s.decision_id = l.id
                          AND s.deleted_at IS NULL
                          AND s.source_fields ? 'ecli'
                        ORDER BY s.source_rank DESC, (s.lang = 'fra') IS TRUE DESC, (s.lang = 'eng') IS TRUE DESC, s.source_uid ASC
                        LIMIT 1
                    ) ds ON true
                    WHERE d.id = l.id
                      AND d.ecli IS NULL
                    RETURNING 1
                )
                SELECT
                    (SELECT count(*) FROM upd) AS updated,
                    (SELECT max(id) FROM lot)  AS last_id
                ",
                &[&after_id, &batch],
            )
            .await?;
        let updated = row.get::<_, i64>(0) as u64;
        match row.get::<_, Option<i64>>(1) {
            Some(last_id) => Ok(Some((updated, last_id))),
            None => Ok(None),
        }
    }

    /// Keyset (par `decisions.id`) des décisions dont le `canonical_ref` reste à
    /// calculer : `canonical_ref IS NULL` et `full_text` présent. Une décision sans clé
    /// exploitable reste `NULL` et sera relue à chaque rerun.
    pub async fn decision_ids_for_canonical_ref_backfill(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.canonical_ref IS NULL
                  AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                ORDER BY d.id
                LIMIT $2
                ",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Keyset (par `decisions.id`) de **toutes** les décisions à `canonical_ref`
    /// recalculable (`full_text` présent), pour un re-dérive forcé : la colonne n'étant
    /// plus `NULL`, le backfill normal ne les revisite pas. Une clé recalculée `None`
    /// laisse l'existante.
    pub async fn decision_ids_for_canonical_ref_recompute(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                ORDER BY d.id
                LIMIT $2
                ",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Écrit en masse le `canonical_ref` calculé en Rust, `unnest` apparié
    /// `(id, key)`. N'inclut que les décisions ayant produit une clé — celles sans clé
    /// restent `NULL`. Renvoie le nombre de lignes mises à jour.
    #[tracing::instrument(name = "db.update_canonical_refs_bulk", skip(self, ids, keys), fields(db.system = "postgresql", n = ids.len()))]
    pub async fn update_canonical_refs_bulk(&self, ids: &[i64], keys: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "
                UPDATE decisions d
                SET canonical_ref = m.canonical_ref
                FROM (SELECT unnest($1::bigint[]) AS id, unnest($2::text[]) AS canonical_ref) m
                WHERE d.id = m.id
                ",
                &[&ids, &keys],
            )
            .await?;
        Ok(n)
    }

    #[tracing::instrument(name = "db.find_ingest_states", skip(self, source_uids), fields(db.system = "postgresql"))]
    pub async fn find_ingest_states(
        &self,
        source_uids: &[String],
    ) -> Result<HashMap<String, ExistingDecisionState>> {
        if source_uids.is_empty() {
            return Ok(HashMap::new());
        }
        // Idempotence par provenance : on lit `source_uid` / `content_checksum` sur
        // `decision_sources`, join `decisions` pour `public_id`. Provenances actives
        // seules : une tombstonée n'apparaît pas ici, le candidat retombe en survivant et
        // `upsert` la ressuscite par `source_uid`.
        let rows = self
            .conn
            .query(
                "
                SELECT
                  ds.source_uid,
                  ds.decision_id,
                  ds.content_checksum,
                  d.public_id
                FROM decision_sources ds
                JOIN decisions d ON d.id = ds.decision_id
                WHERE ds.source_uid = ANY($1) AND ds.deleted_at IS NULL
                ",
                &[&source_uids],
            )
            .await?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let source_uid: String = row.get(0);
            out.insert(
                source_uid.clone(),
                ExistingDecisionState {
                    id: row.get(1),
                    source_uid,
                    content_checksum: row.get(2),
                    public_id: row.get(3),
                },
            );
        }
        Ok(out)
    }

    /// The languages the base holds for each CELEX, tombstoned included, each with the
    /// size in bytes of the text it wrote, and the source fields of one of them.
    #[tracing::instrument(name = "db.cjue_languages", skip(self, celexes), fields(db.system = "postgresql"))]
    pub async fn cjue_languages(&self, celexes: &[String]) -> Result<HashMap<String, CjueHeld>> {
        let uids: Vec<String> = celexes
            .iter()
            .flat_map(|c| {
                lj_vocab::languages::EU_OFFICIAL_LANGUAGES
                    .iter()
                    .map(move |l| format!("cjue/{c}/{l}"))
            })
            .collect();
        let rows = self
            .conn
            .query(
                "SELECT split_part(s.source_uid, '/', 2), split_part(s.source_uid, '/', 3), \
                        CASE WHEN row_number() OVER (PARTITION BY split_part(s.source_uid, '/', 2) \
                                                     ORDER BY (s.lang = 'fra') IS TRUE DESC, s.source_uid) = 1 \
                             THEN s.source_fields END, \
                        coalesce((SELECT octet_length(t.full_text) FROM decision_text t \
                                  WHERE t.decision_id = s.decision_id AND t.source_uid = s.source_uid), 0) \
                 FROM decision_sources s WHERE s.source_uid = ANY($1)",
                &[&uids],
            )
            .await?;
        let mut out: HashMap<String, CjueHeld> = HashMap::new();
        for r in rows {
            let entry = out.entry(r.get(0)).or_insert((Vec::new(), Value::Null));
            entry.0.push((r.get(1), r.get(3)));
            if let Some(fields) = r.get::<_, Option<Value>>(2) {
                entry.1 = fields;
            }
        }
        Ok(out)
    }

    /// The decision an other language of the same judgment already carries:
    /// the CJUE by its CELEX, else by an ECLI naming a single decision, the CEDH
    /// by its ECLI, juportal by the ECLI its French and Dutch pages share. `None`
    /// when none, or when the ECLI names several decisions or, at the CEDH, one
    /// that already has `lang`.
    #[tracing::instrument(name = "db.find_language_sibling", skip(self, decision), fields(db.system = "postgresql"))]
    pub(super) async fn find_language_sibling(
        &self,
        decision: &lj_core::decision::Decision,
        lang: Option<&str>,
    ) -> Result<Option<i64>> {
        if let Some(rest) = decision.source_uid.strip_prefix("cjue/") {
            let celex = rest.split('/').next().unwrap_or(rest);
            let uids: Vec<String> = lj_vocab::languages::EU_OFFICIAL_LANGUAGES
                .iter()
                .map(|l| format!("cjue/{celex}/{l}"))
                .filter(|u| *u != decision.source_uid)
                .collect();
            let row = self
                .conn
                .query_one(
                    "SELECT min(decision_id) FROM decision_sources WHERE source_uid = ANY($1)",
                    &[&uids],
                )
                .await?;
            if let Some(id) = row.get::<_, Option<i64>>(0) {
                return Ok(Some(id));
            }
            // CELLAR lists some works twice under a suffixed CELEX (62021CO0121_1),
            // same ECLI and same text.
            let Some(ecli) = &decision.ecli else {
                return Ok(None);
            };
            let rows = self
                .conn
                .query(
                    "SELECT id FROM decisions \
                     WHERE ecli = $1 AND jurisdiction_type = 'CJUE' AND deleted_at IS NULL LIMIT 2",
                    &[ecli],
                )
                .await?;
            return Ok(match rows.as_slice() {
                [r] => Some(r.get(0)),
                _ => None,
            });
        }
        if let Some(rest) = decision.source_uid.strip_prefix("juportal/") {
            let ecli = rest.strip_suffix("/nl").unwrap_or(rest);
            let uids: Vec<String> = [format!("juportal/{ecli}"), format!("juportal/{ecli}/nl")]
                .into_iter()
                .filter(|u| *u != decision.source_uid)
                .collect();
            let row = self
                .conn
                .query_one(
                    "SELECT min(decision_id) FROM decision_sources WHERE source_uid = ANY($1)",
                    &[&uids],
                )
                .await?;
            return Ok(row.get(0));
        }
        if let Some((reference, _)) = decision
            .source_uid
            .strip_prefix("comeu/")
            .and_then(|rest| rest.rsplit_once('/'))
        {
            let uids: Vec<String> = lj_vocab::languages::EU_OFFICIAL_LANGUAGES
                .iter()
                .map(|l| format!("comeu/{reference}/{l}"))
                .filter(|u| *u != decision.source_uid)
                .collect();
            let row = self
                .conn
                .query_one(
                    "SELECT min(decision_id) FROM decision_sources WHERE source_uid = ANY($1)",
                    &[&uids],
                )
                .await?;
            return Ok(row.get(0));
        }
        if let Some(uids) = decisia_siblings(&decision.source_uid) {
            let row = self
                .conn
                .query_one(
                    "SELECT min(decision_id) FROM decision_sources WHERE source_uid = ANY($1)",
                    &[&uids],
                )
                .await?;
            return Ok(row.get(0));
        }
        let (true, Some(ecli)) = (decision.source_uid.starts_with("cedh/"), &decision.ecli) else {
            return Ok(None);
        };
        let rows = self
            .conn
            .query(
                "SELECT d.id, EXISTS (SELECT 1 FROM decision_sources s \
                   WHERE s.decision_id = d.id AND s.lang IS NOT DISTINCT FROM $2 AND s.deleted_at IS NULL \
                     AND s.source_uid <> $3) \
                 FROM decisions d \
                 WHERE d.ecli = $1 AND d.jurisdiction_type = 'CEDH' AND d.deleted_at IS NULL LIMIT 2",
                &[ecli, &lang, &decision.source_uid],
            )
            .await?;
        Ok(match rows.as_slice() {
            [r] if !r.get::<_, bool>(1) => Some(r.get(0)),
            _ => None,
        })
    }

    /// Where a provenance the base does not know goes: another language of the
    /// same judgment, then the decision at its address.
    pub async fn identity_of(
        &self,
        decision: &lj_core::decision::Decision,
        extracted: Option<&ExtractedFields>,
        source_fields: &Value,
    ) -> Result<Identity> {
        let lang = source_fields.get("lang").and_then(Value::as_str);
        match self.find_language_sibling(decision, lang).await? {
            Some(id) => Ok(Identity::Attach(id)),
            None => self.resolve_identity(decision, extracted).await,
        }
    }

    /// The decisions at the address of a record (same court, same date, a
    /// docket in common), told apart by
    /// [`lj_core::identity::choose`]. A decision already holding a provenance of
    /// the incoming source is never a target.
    #[tracing::instrument(name = "db.resolve_identity", skip_all, fields(db.system = "postgresql", uid = %decision.source_uid))]
    async fn resolve_identity(
        &self,
        decision: &lj_core::decision::Decision,
        extracted: Option<&ExtractedFields>,
    ) -> Result<Identity> {
        let Some(e) = extracted else {
            return Ok(Identity::Create);
        };
        let (Some(court), Some(date)) = (e.jurisdiction.as_ref(), e.date_lecture) else {
            return Ok(Identity::Create);
        };
        let court = self.court_code(court).await?;
        let source = source_from_source_uid(&decision.source_uid);
        if source == LEGALIS {
            return self.legalis_identity(decision, &court, date).await;
        }
        if e.docket_numbers.is_empty() {
            return self.created_or_legalis(decision, &court, date).await;
        }
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.ecli, d.publication_codes, d.seat_paths, d.solution_uid, \
                   ARRAY(SELECT l.target_date::text FROM decision_links l \
                         WHERE l.decision_id = d.id AND l.target_date IS NOT NULL \
                           AND l.link_type IN ('POURVOI_CONTRE', 'APPEL_DE')), \
                   ARRAY(SELECT s.source FROM decision_sources s \
                         WHERE s.decision_id = d.id AND s.deleted_at IS NULL AND s.source_uid <> $4) \
                 FROM decisions d \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_code = $1 AND d.date_lecture = $2 \
                   AND lj_docket_keys(d.docket_numbers) && lj_docket_keys($3) \
                 ORDER BY d.id",
                &[&court, &date, &e.docket_numbers, &decision.source_uid],
            )
            .await?;
        let (mut ids, mut traits, mut held_by_source) = (Vec::new(), Vec::new(), Vec::new());
        for r in rows {
            let sources: Vec<String> = r.get(6);
            if sources.is_empty() {
                continue;
            }
            let id: i64 = r.get(0);
            if sources.iter().any(|s| same_publisher(s, source)) {
                held_by_source.push(id);
                continue;
            }
            ids.push(id);
            traits.push(Traits {
                ecli: r.get(1),
                publication_codes: r.get(2),
                seat_paths: r.get(3),
                solution_uid: r.get(4),
                attacked_dates: r.get(5),
            });
        }
        let incoming = Traits {
            ecli: decision.ecli.clone(),
            publication_codes: e.publication_codes.clone(),
            seat_paths: e.seat_paths.clone(),
            solution_uid: e.solution_uid.clone(),
            attacked_dates: e
                .decision_links
                .iter()
                .flatten()
                .filter(|l| matches!(l.link_type.as_str(), "POURVOI_CONTRE" | "APPEL_DE"))
                .filter_map(|l| l.address.date.clone())
                .collect(),
        };
        match choose(&incoming, &traits) {
            Choice::One(i) => return Ok(Identity::Attach(ids[i])),
            Choice::Tied(tied) => {
                let tied: Vec<i64> = tied.into_iter().map(|i| ids[i]).collect();
                let mut best: Option<(f64, i64)> = None;
                for (id, text) in self.read_texts(&tied).await? {
                    let identity = text_identity(&decision.texte_integral_clean, &text);
                    if best.is_none_or(|(b, _)| identity > b) {
                        best = Some((identity, id));
                    }
                }
                if let Some((identity, id)) = best {
                    if identity >= TIED_TEXT_IDENTITY {
                        return Ok(Identity::Attach(id));
                    }
                }
            }
            Choice::Neither => {}
        }
        if !is_dila_judicial(source) || held_by_source.is_empty() {
            return self.created_or_legalis(decision, &court, date).await;
        }
        Ok(
            match self
                .best_containment(&decision.texte_integral_clean, &held_by_source)
                .await?
            {
                Some((c, id)) if c >= DILA_COPY_CONTAINMENT => Identity::Copy(id),
                Some((c, id)) if c > DILA_DISTINCT_CONTAINMENT => Identity::Hold(id),
                _ => self.created_or_legalis(decision, &court, date).await?,
            },
        )
    }

    /// A Legalis record: the decision at its court and date whose text holds it, one Legalis
    /// already holds excepted.
    async fn legalis_identity(
        &self,
        decision: &lj_core::decision::Decision,
        court: &str,
        date: NaiveDate,
    ) -> Result<Identity> {
        let ids: Vec<i64> = self
            .conn
            .query(
                "SELECT d.id FROM decisions d \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_code = $1 AND d.date_lecture = $2 \
                   AND EXISTS (SELECT 1 FROM decision_sources s \
                               WHERE s.decision_id = d.id AND s.deleted_at IS NULL) \
                   AND NOT EXISTS (SELECT 1 FROM decision_sources s \
                                   WHERE s.decision_id = d.id AND s.deleted_at IS NULL AND s.source = $3)",
                &[&court, &date, &LEGALIS],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        Ok(
            match self
                .best_containment(&decision.texte_integral_clean, &ids)
                .await?
            {
                Some((c, id)) if c >= LEGALIS_COPY_CONTAINMENT => Identity::Attach(id),
                Some((c, id)) if c > LEGALIS_DISTINCT_CONTAINMENT => Identity::Hold(id),
                _ => Identity::Create,
            },
        )
    }

    /// The decision Legalis alone holds at this court and date whose text the record holds, else
    /// a new one.
    async fn created_or_legalis(
        &self,
        decision: &lj_core::decision::Decision,
        court: &str,
        date: NaiveDate,
    ) -> Result<Identity> {
        let ids: Vec<i64> = self
            .conn
            .query(
                "SELECT d.id FROM decisions d \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_code = $1 AND d.date_lecture = $2 \
                   AND EXISTS (SELECT 1 FROM decision_sources s \
                               WHERE s.decision_id = d.id AND s.deleted_at IS NULL AND s.source = $3) \
                   AND NOT EXISTS (SELECT 1 FROM decision_sources s \
                                   WHERE s.decision_id = d.id AND s.deleted_at IS NULL AND s.source <> $3)",
                &[&court, &date, &LEGALIS],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        if ids.is_empty() {
            return Ok(Identity::Create);
        }
        Ok(
            match self
                .best_containment(&decision.texte_integral_clean, &ids)
                .await?
            {
                Some((c, id)) if c >= LEGALIS_COPY_CONTAINMENT => Identity::Attach(id),
                _ => Identity::Create,
            },
        )
    }

    /// Of `ids`, the decision whose text shares the most with `text`, and that share.
    async fn best_containment(&self, text: &str, ids: &[i64]) -> Result<Option<(f64, i64)>> {
        let mut best: Option<(f64, i64)> = None;
        for (id, held) in self.read_texts(ids).await? {
            let c = text_containment(text, &held);
            if best.is_none_or(|(b, _)| c > b) {
                best = Some((c, id));
            }
        }
        Ok(best)
    }

    /// Those of `ids` whose fields come from several provenances.
    #[tracing::instrument(name = "db.decisions_with_several_provenances", skip_all, fields(db.system = "postgresql", n = ids.len()))]
    pub async fn decisions_with_several_provenances(&self, ids: &[i64]) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "SELECT decision_id FROM decision_sources \
                 WHERE decision_id = ANY($1) AND deleted_at IS NULL AND source_rank > 0 \
                   AND source_fields IS NOT NULL \
                 GROUP BY decision_id HAVING count(*) > 1",
                &[&ids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Every decision whose fields come from several provenances, by id.
    #[tracing::instrument(name = "db.all_decisions_with_several_provenances", skip(self), fields(db.system = "postgresql"))]
    pub async fn all_decisions_with_several_provenances(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "SELECT decision_id FROM decision_sources \
                 WHERE deleted_at IS NULL AND source_rank > 0 AND source_fields IS NOT NULL \
                 GROUP BY decision_id HAVING count(*) > 1 ORDER BY decision_id",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// The `source_uid`s the base holds among these, tombstoned included.
    #[tracing::instrument(name = "db.known_source_uids", skip_all, fields(db.system = "postgresql", n = uids.len()))]
    pub async fn known_source_uids(&self, uids: &[String]) -> Result<HashSet<String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid FROM decision_sources WHERE source_uid = ANY($1)",
                &[&uids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Every Judilibre provenance: `source_uid`, whether it is tombstoned, and
    /// the `jurisdiction` and `decision_date` of its source fields.
    #[tracing::instrument(name = "db.judilibre_provenances", skip(self), fields(db.system = "postgresql"))]
    pub async fn judilibre_provenances(&self) -> Result<Vec<JudilibreProvenance>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, deleted_at IS NOT NULL, source_fields->>'jurisdiction', \
                        source_fields->>'decision_date' \
                   FROM decision_sources WHERE source = 'judilibre'",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| JudilibreProvenance {
                source_uid: r.get(0),
                tombstoned: r.get(1),
                jurisdiction: r.get(2),
                decision_date: r.get(3),
            })
            .collect())
    }

    /// Every active `juradm-site` provenance: its `source_uid` and the bucket key of the file it
    /// was read from.
    #[tracing::instrument(name = "db.juradm_site_files", skip(self), fields(db.system = "postgresql"))]
    pub async fn juradm_site_files(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, source_fields->>'file' FROM decision_sources \
                  WHERE source = 'juradm-site' AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Every active provenance of `source`: its decision, `source_uid`, checksum and payload format.
    #[tracing::instrument(name = "db.source_provenances", skip(self), fields(db.system = "postgresql"))]
    pub async fn source_provenances(
        &self,
        source: &str,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT decision_id, source_uid, content_checksum, payload_format \
                   FROM decision_sources WHERE source = $1 AND deleted_at IS NULL",
                &[&source],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// The Conseil constitutionnel's decision pages, among these, that an active provenance names.
    #[tracing::instrument(name = "db.constit_pages_held", skip_all, fields(db.system = "postgresql", n = urls.len()))]
    pub async fn constit_pages_held(&self, urls: &[String]) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_fields->'META_JURI_CONSTIT'->>'URL_CC' FROM decision_sources \
                  WHERE source IN ('dila-constit', 'cc-site') AND deleted_at IS NULL \
                    AND source_fields->'META_JURI_CONSTIT'->>'URL_CC' = ANY($1)",
                &[&urls],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// The active `source_uid`s the base holds among these.
    #[tracing::instrument(name = "db.active_source_uids", skip_all, fields(db.system = "postgresql", n = uids.len()))]
    pub async fn active_source_uids(&self, uids: &[String]) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid FROM decision_sources WHERE source_uid = ANY($1) AND deleted_at IS NULL",
                &[&uids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    #[tracing::instrument(name = "db.decision_id_by_public_id", skip(self), fields(db.system = "postgresql"))]
    pub async fn decision_id_by_public_id(&self, public_id: &str) -> Result<Option<i64>> {
        let row = self
            .conn
            .query_opt(
                "SELECT id FROM decisions WHERE public_id = $1 AND deleted_at IS NULL",
                &[&public_id],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Id, court and date of a live decision, by its public id.
    #[tracing::instrument(name = "db.decision_address", skip(self), fields(db.system = "postgresql"))]
    pub async fn decision_address(&self, public_id: &str) -> Result<Option<DecisionAddress>> {
        let row = self
            .conn
            .query_opt(
                "SELECT id, jurisdiction_code, date_lecture FROM decisions \
                 WHERE public_id = $1 AND deleted_at IS NULL",
                &[&public_id],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1), r.get(2))))
    }

    /// Rank of the decision's highest active provenance.
    #[tracing::instrument(name = "db.top_source_rank", skip(self), fields(db.system = "postgresql"))]
    pub async fn top_source_rank(&self, decision_id: i64) -> Result<Option<i16>> {
        let row = self
            .conn
            .query_one(
                "SELECT max(source_rank) FROM decision_sources \
                 WHERE decision_id = $1 AND deleted_at IS NULL",
                &[&decision_id],
            )
            .await?;
        Ok(row.get(0))
    }

    /// Active provenances (re)ingested in the last `days` days, of `sources` or
    /// of all sources when empty.
    #[tracing::instrument(name = "db.recent_provenance_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn recent_provenance_uids(
        &self,
        days: i32,
        sources: &[String],
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid FROM decision_sources \
                 WHERE deleted_at IS NULL AND ingested_at > now() - make_interval(days => $1) \
                   AND (cardinality($2::text[]) = 0 OR source = ANY($2)) \
                 ORDER BY source_uid",
                &[&days, &sources],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Moves one provenance onto `target`, and drops the decision it leaves when
    /// it held no other.
    #[tracing::instrument(name = "db.move_provenance", skip(self), fields(db.system = "postgresql"))]
    pub async fn move_provenance(&self, source_uid: &str, target: i64) -> Result<i64> {
        self.conn.batch_execute("BEGIN").await?;
        let res = async {
            let from: i64 = self
                .conn
                .query_one(
                    "SELECT decision_id FROM decision_sources WHERE source_uid = $1",
                    &[&source_uid],
                )
                .await?
                .get(0);
            self.conn
                .execute(
                    "UPDATE decision_sources SET decision_id = $2 WHERE source_uid = $1",
                    &[&source_uid, &target],
                )
                .await?;
            self.reconcile(target).await?;
            self.reconcile(from).await?;
            Ok(from)
        }
        .await;
        match res {
            Ok(from) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(from)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Deletes a provenance outright, and its decision when it held no other.
    #[tracing::instrument(name = "db.delete_provenance", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_provenance(&self, source_uid: &str) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let res = async {
            let row = self
                .conn
                .query_one(
                    "DELETE FROM decision_sources WHERE source_uid = $1 RETURNING decision_id",
                    &[&source_uid],
                )
                .await?;
            self.reconcile(row.get(0)).await
        }
        .await;
        match res {
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

    /// What resolving these provenances again needs.
    #[tracing::instrument(name = "db.provenances_to_resolve", skip_all, fields(db.system = "postgresql", n = uids.len()))]
    pub async fn provenances_to_resolve(
        &self,
        uids: &[String],
    ) -> Result<Vec<ProvenanceToResolve>> {
        let rows = self
            .conn
            .query(
                &format!("SELECT s.decision_id, s.source_uid, coalesce(s.source_fields, 'null'::jsonb), \
                   coalesce({text}, ''), \
                   EXISTS (SELECT 1 FROM decision_sources o \
                           WHERE o.decision_id = s.decision_id AND o.deleted_at IS NULL \
                             AND o.source_uid <> s.source_uid) \
                 FROM decision_sources s JOIN decisions d ON d.id = s.decision_id \
                 WHERE s.source_uid = ANY($1) AND d.deleted_at IS NULL", text = super::pick::text("p.full_text", "d")),
                &[&uids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| ProvenanceToResolve {
                decision_id: r.get(0),
                source_uid: r.get(1),
                source_fields: r.get(2),
                read_text: r.get(3),
                shared: r.get(4),
            })
            .collect())
    }

    /// The code `ensure_jurisdictions` gives this row, read without writing it.
    pub(super) async fn court_code(&self, j: &JurisdictionRow) -> Result<String> {
        let row = self
            .conn
            .query_opt(
                "SELECT code FROM jurisdiction WHERE source_code = $1",
                &[&j.source_code],
            )
            .await?;
        Ok(match row {
            Some(r) => r.get(0),
            None => {
                canonical_jurisdiction_code(&j.jurisdiction_type, j.city.as_deref(), &j.source_code)
            }
        })
    }

    async fn read_texts(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT t.decision_id, t.full_text FROM decision_text t \
                     WHERE t.decision_id = ANY($1) AND {read} ORDER BY t.decision_id",
                    read = super::pick::read("t")
                ),
                &[&ids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Retrait d'une provenance, **toujours par `source_uid`** et jamais par
    /// identité canonique : tombstone la ligne `decision_sources`, puis
    /// `reconcile(decision_id)` recalcule la décision. Les deux sont atomiques et sous
    /// verrou (`pg_advisory_xact_lock` sur `decision_id`), les appelants tournant en
    /// autocommit. Le squelette (id, public_id, identité) reste : un ré-update se
    /// rejoint par `source_uid`. `source_uid` inconnu → no-op idempotent (`false`).
    #[tracing::instrument(name = "db.delete", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete(&self, source_uid: &str) -> Result<bool> {
        self.conn.batch_execute("BEGIN").await?;
        let res = self.delete_locked(source_uid).await;
        match res {
            Ok(deleted) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(deleted)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Corps transactionnel de [`delete`](Self::delete) : résout la provenance, pose
    /// le verrou xact sur `decision_id`, tombstone, `reconcile`. À exécuter dans la
    /// transaction ouverte par l'appelant.
    pub(super) async fn delete_locked(&self, source_uid: &str) -> Result<bool> {
        let row = self
            .conn
            .query_opt(
                "SELECT decision_id FROM decision_sources WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        let Some(row) = row else {
            // Provenance inconnue : rien à retirer.
            return Ok(false);
        };
        let decision_id: i64 = row.get(0);
        // Verrou par décision (auto-relâché en fin de transaction) : sérialise retrait
        // et upsert concurrents sur la même décision.
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&decision_id])
            .await?;
        self.conn
            .execute(
                "UPDATE decision_sources SET deleted_at = NOW() WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        self.reconcile(decision_id).await?;
        Ok(true)
    }

    /// `reconcile(decision_id)` — recalcule la décision canonique comme **fonction
    /// pure de ses `decision_sources`**. À appeler après toute mutation de provenance,
    /// sous verrou, dans la même transaction.
    ///
    /// 1. **0 provenance** → hard-delete la décision (cascade chunks/refs).
    /// 2. **autorité** = `source_rank` max (active ou tombstonée), départagée par
    ///    rendition FR-prioritaire puis `source_uid` croissant.
    /// 3. **autorité active** → décision active, `deleted_at = NULL`. `reconcile` ne
    ///    re-dérive jamais le texte : il est posé par l'ingest de la provenance.
    /// 4. **autorité tombstonée** → vidé : `full_text`/`summary` NULL,
    ///    `deleted_at = NOW()`, chunks + payload brut + mots-clés supprimés, même si
    ///    une source inférieure est encore active.
    #[tracing::instrument(name = "db.reconcile", skip(self), fields(db.system = "postgresql"))]
    pub async fn reconcile(&self, decision_id: i64) -> Result<()> {
        // Autorité = rang max, active ou tombstonée. Son `deleted_at` décide de l'état
        // de la décision.
        let authority = self
            .conn
            .query_opt(
                "SELECT deleted_at IS NULL AS active \
                 FROM decision_sources WHERE decision_id = $1 \
                 ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, (lang = 'eng') IS TRUE DESC, source_uid ASC LIMIT 1",
                &[&decision_id],
            )
            .await?;

        let Some(authority) = authority else {
            // 0 provenance → hard-delete (cascade chunks/refs/full_text).
            self.conn
                .execute("DELETE FROM decisions WHERE id = $1", &[&decision_id])
                .await?;
            return Ok(());
        };

        let authority_active: bool = authority.get("active");
        if authority_active {
            // Autorité active → décision active. `full_text`/métadonnées déjà posés par
            // l'ingest de l'autorité ; on lève juste un éventuel vide.
            self.conn
                .execute(
                    "UPDATE decisions SET deleted_at = NULL \
                     WHERE id = $1 AND deleted_at IS NOT NULL",
                    &[&decision_id],
                )
                .await?;
            return Ok(());
        }

        // Autorité tombstonée → vidé : on retire tout porteur de données perso (texte,
        // chunks+embeddings, payload brut, résumé).
        self.conn
            .execute(
                "UPDATE decisions SET deleted_at = NOW() WHERE id = $1",
                &[&decision_id],
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM decision_chunks WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM decision_text WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM decision_full_text WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        Ok(())
    }

    /// Prédicat d'un tombstone **orphelin** (retrait total, purgeable) : décision
    /// retirée, vidée (`full_text IS NULL`) et sans aucune provenance active — une
    /// provenance active la ressusciterait via [`reconcile`](Self::reconcile). Partagé
    /// par le comptage, l'échantillon et le DELETE, qui visent la même cible.
    const ORPHAN_TOMBSTONE_WHERE: &'static str = "d.deleted_at IS NOT NULL \
         AND NOT EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id) \
         AND NOT EXISTS ( \
           SELECT 1 FROM decision_sources s \
           WHERE s.decision_id = d.id AND s.deleted_at IS NULL)";

    /// Compte les tombstones orphelins (cf. [`Self::ORPHAN_TOMBSTONE_WHERE`]).
    pub async fn count_orphan_tombstones(&self) -> Result<i64> {
        let sql = format!(
            "SELECT count(*) FROM decisions d WHERE {}",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        Ok(self.conn.query_one(&sql, &[]).await?.get(0))
    }

    /// Échantillon de `source_uid` d'orphelins (contrôle avant purge).
    pub async fn sample_orphan_tombstones(&self, limit: i64) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT (SELECT source_uid FROM decision_sources \
                     WHERE decision_id = d.id LIMIT 1) \
             FROM decisions d WHERE {} LIMIT $1",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        let rows = self.conn.query(&sql, &[&limit]).await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.get::<_, Option<String>>(0))
            .collect())
    }

    /// Hard-delete des tombstones orphelins (cascade FK). Renvoie le nombre
    /// supprimé. La clause re-vérifie l'absence de provenance active au moment du
    /// DELETE : une réactivation concurrente sort la ligne de la cible.
    pub async fn purge_orphan_tombstones(&self) -> Result<u64> {
        let sql = format!(
            "DELETE FROM decisions d WHERE {}",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        Ok(self.conn.execute(&sql, &[]).await?)
    }

    /// Clusters de décisions actives à **fusionner** (faux splits cross-source) :
    /// même `canonical_ref`, ≥2 décisions actives, **sources disjointes**. La
    /// disjonction garantit qu'aucune fusion ne recolle deux provenances same-source ;
    /// les affaires sérielles same-source restent donc scindées. Renvoie, par cluster,
    /// `(decision_id, max_source_rank)` triés rang décroissant puis id croissant :
    /// l'appelant garde `[0]` et fusionne les suivants dedans.
    #[tracing::instrument(name = "db.fetch_cross_source_merge_groups", skip(self), fields(db.system = "postgresql"))]
    pub async fn fetch_cross_source_merge_groups(&self) -> Result<Vec<Vec<(i64, i32)>>> {
        let rows = self
            .conn
            .query(
                "
                WITH prov AS (
                    SELECT d.id AS decision_id, d.canonical_ref, s.source, s.source_rank
                    FROM decisions d
                    JOIN decision_sources s ON s.decision_id = d.id AND s.deleted_at IS NULL
                    WHERE d.deleted_at IS NULL AND d.canonical_ref IS NOT NULL
                ),
                per_ref AS (
                    SELECT canonical_ref,
                           count(DISTINCT decision_id)            AS n_dec,
                           count(DISTINCT source)                 AS n_src,
                           count(DISTINCT (decision_id, source))  AS n_dec_src
                    FROM prov GROUP BY canonical_ref
                ),
                safe AS (
                    -- ≥2 décisions ET sources disjointes (n_src = n_dec_src ⟺ aucune
                    -- source répétée entre décisions du cluster).
                    SELECT canonical_ref FROM per_ref
                    WHERE n_dec > 1 AND n_src = n_dec_src
                )
                SELECT p.canonical_ref, p.decision_id, max(p.source_rank)::int AS max_rank
                FROM prov p JOIN safe USING (canonical_ref)
                GROUP BY p.canonical_ref, p.decision_id
                ORDER BY p.canonical_ref, max_rank DESC, p.decision_id
                ",
                &[],
            )
            .await?;
        // Regroupe les lignes (déjà triées par canonical_ref) en clusters.
        let mut groups: Vec<Vec<(i64, i32)>> = Vec::new();
        let mut cur_ref: Option<String> = None;
        for row in rows {
            let cref: String = row.get(0);
            let pair = (row.get::<_, i64>(1), row.get::<_, i32>(2));
            if cur_ref.as_deref() == Some(cref.as_str()) {
                groups.last_mut().expect("cluster ouvert").push(pair);
            } else {
                groups.push(vec![pair]);
                cur_ref = Some(cref);
            }
        }
        Ok(groups)
    }

    /// Merges `loser_id` into `keeper_id`: repoints every provenance of the loser,
    /// active and tombstoned, to the keeper, then `reconcile`s both, which hard-deletes
    /// the loser left without provenance. Transactional, under ordered locks against
    /// deadlocks. Safe only when the sources are disjoint, which
    /// [`fetch_cross_source_merge_groups`](Self::fetch_cross_source_merge_groups)
    /// guarantees. The keeper's content is kept, nothing is re-embedded. No-op when
    /// `keeper_id == loser_id`.
    #[tracing::instrument(name = "db.merge_into", skip(self), fields(db.system = "postgresql"))]
    pub async fn merge_into(&self, keeper_id: i64, loser_id: i64) -> Result<()> {
        if keeper_id == loser_id {
            return Ok(());
        }
        self.conn.batch_execute("BEGIN").await?;
        let res = self.merge_into_locked(keeper_id, loser_id).await;
        match res {
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

    /// Corps transactionnel de [`merge_into`](Self::merge_into) : verrous ordonnés,
    /// re-pointage, reconcile des deux décisions. À exécuter dans la transaction
    /// ouverte par l'appelant.
    async fn merge_into_locked(&self, keeper_id: i64, loser_id: i64) -> Result<()> {
        let (lo, hi) = if keeper_id < loser_id {
            (keeper_id, loser_id)
        } else {
            (loser_id, keeper_id)
        };
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&lo])
            .await?;
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&hi])
            .await?;
        self.conn
            .execute(
                "UPDATE decision_sources SET decision_id = $1 WHERE decision_id = $2",
                &[&keeper_id, &loser_id],
            )
            .await?;
        self.reconcile(keeper_id).await?;
        self.reconcile(loser_id).await?;
        Ok(())
    }

    /// Absorbs `copy` into `keeper`, the decision it was published from under another
    /// number: what pointed at the copy follows its provenances.
    #[tracing::instrument(name = "db.absorb_into", skip(self), fields(db.system = "postgresql"))]
    pub async fn absorb_into(&self, copy: i64, keeper: i64) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let res = self.absorb_into_locked(copy, keeper).await;
        match res {
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

    async fn absorb_into_locked(&self, copy: i64, keeper: i64) -> Result<()> {
        self.conn
            .execute(
                "WITH links AS (
                     UPDATE decision_links SET target_decision_id = $1 WHERE target_decision_id = $2),
                 cites AS (
                     UPDATE case_citation SET target_decision_id = $1 WHERE target_decision_id = $2),
                 text_cites AS (
                     UPDATE text_case_citation SET target_decision_id = $1 WHERE target_decision_id = $2)
                 UPDATE decision_renvoi SET target_decision_id = $1 WHERE target_decision_id = $2",
                &[&keeper, &copy],
            )
            .await?;
        self.merge_into_locked(keeper, copy).await
    }

    /// Absorbs `copy` into `keeper`, the same decision in another language, as
    /// [`absorb_into`](Self::absorb_into) does. A text of a language the keeper lacks follows its
    /// provenance with its summary, and takes the keeper's columns as `decision_text_put` writes
    /// them. Returns the texts moved.
    #[tracing::instrument(name = "db.absorb_language_into", skip(self), fields(db.system = "postgresql"))]
    pub async fn absorb_language_into(&self, copy: i64, keeper: i64) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let res = async {
            let moved = self
                .conn
                .execute(
                    "UPDATE decision_text t SET decision_id = d.id, counted = FALSE, \
                       text_status = CASE WHEN t.lang = d.lang THEN 'authentic' ELSE 'official' END, \
                       search_title = d.search_title, jurisdiction_type = d.jurisdiction_type, \
                       jurisdiction_code = d.jurisdiction_code, seat_paths = d.seat_paths, \
                       formation_paths = d.formation_paths, legal_instruments = d.legal_instruments, \
                       legal_article_composite = d.legal_article_composite, \
                       publication_codes = d.publication_codes, legal_domain_uid = d.legal_domain_uid, \
                       solution_uid = d.solution_uid, office_uid = d.office_uid, \
                       procedure_uid = d.procedure_uid, docket_numbers = d.docket_numbers, \
                       ecli = d.ecli, date_lecture = d.date_lecture, deleted_at = d.deleted_at, \
                       kind = d.kind \
                     FROM decisions d \
                     WHERE d.id = $1 AND t.decision_id = $2 \
                       AND NOT EXISTS (SELECT 1 FROM decision_text k \
                                       WHERE k.decision_id = $1 AND k.lang = t.lang)",
                    &[&keeper, &copy],
                )
                .await?;
            self.absorb_into_locked(copy, keeper).await?;
            Ok(moved)
        }
        .await;
        match res {
            Ok(moved) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(moved)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// The decisions of `court` a publisher holds under two slug families, as (keeper, twin):
    /// the decision of a `kept_prefix` slug and that of the `dup_prefix` slug of the same day
    /// and number.
    #[tracing::instrument(name = "db.juricaf_twin_pairs", skip(self), fields(db.system = "postgresql"))]
    pub async fn juricaf_twin_pairs(
        &self,
        court: &str,
        dup_prefix: &str,
        kept_prefix: &str,
    ) -> Result<Vec<(i64, i64)>> {
        let rows = self
            .conn
            .query(
                "WITH held AS MATERIALIZED (
                     SELECT s.decision_id, substr(s.source_uid, length('juricaf/') + 1) AS slug
                     FROM decisions d JOIN decision_sources s ON s.decision_id = d.id
                     WHERE d.jurisdiction_code = $1 AND d.deleted_at IS NULL
                       AND s.source = 'juricaf' AND s.deleted_at IS NULL)
                 SELECT k.decision_id, t.decision_id
                 FROM held t JOIN held k ON k.slug = $3 || substr(t.slug, length($2) + 1)
                 WHERE starts_with(t.slug, $2) AND k.decision_id <> t.decision_id
                 ORDER BY 1, 2",
                &[&court, &dup_prefix, &kept_prefix],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Each decision of `court` whose every provenance is `source`'s, with the decisions of the
    /// court that hold none of `source`, of its day, sharing a docket key with it.
    #[tracing::instrument(name = "db.single_source_twins", skip(self), fields(db.system = "postgresql"))]
    pub async fn single_source_twins(
        &self,
        court: &str,
        source: &str,
    ) -> Result<Vec<(i64, Vec<i64>)>> {
        let rows = self
            .conn
            .query(
                "WITH d AS MATERIALIZED (
                     SELECT d.id, d.date_lecture, lj_docket_keys(d.docket_numbers) AS k,
                            bool_and(s.source = $2) AS only, bool_or(s.source = $2) AS has
                     FROM decisions d
                     JOIN decision_sources s ON s.decision_id = d.id AND s.deleted_at IS NULL
                     WHERE d.jurisdiction_code = $1 AND d.deleted_at IS NULL
                     GROUP BY d.id)
                 SELECT c.id, array_agg(o.id ORDER BY o.id)
                 FROM d c JOIN d o ON o.date_lecture = c.date_lecture AND o.k && c.k AND NOT o.has
                 WHERE c.only
                 GROUP BY c.id
                 ORDER BY c.id",
                &[&court, &source],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Each Judilibre record without an operative part, beside a decision of its court, day
    /// and chamber that has one and speaks of a joinder, on a day fed in the last `days`.
    #[tracing::instrument(name = "db.joinder_copy_pairs", skip(self), fields(db.system = "postgresql"))]
    pub async fn joinder_copy_pairs(&self, days: i32) -> Result<Vec<JoinderPair>> {
        // OFFSET 0 keeps one index lookup per copy: the planner overestimates the copies
        // and would hash every Judilibre record instead.
        let rows = self
            .conn
            .query(
                &format!(
                    r#"WITH copy AS MATERIALIZED (
                         SELECT ds.decision_id, ds.source_fields->>'chamber' AS chamber,
                                d.jurisdiction_code, d.date_lecture, d.docket_numbers
                         FROM decision_sources ds
                         JOIN decisions d ON d.id = ds.decision_id AND d.deleted_at IS NULL
                         WHERE ds.source = 'judilibre' AND ds.deleted_at IS NULL
                           AND NOT ds.source_fields->'sections' @> '[{{"kind":"dispositif"}}]'
                           AND NOT EXISTS (SELECT 1 FROM decision_sources o
                                           WHERE o.decision_id = ds.decision_id AND o.deleted_at IS NULL
                                             AND o.source_fields->'sections' @> '[{{"kind":"dispositif"}}]')),
                     fresh AS MATERIALIZED (
                         SELECT d.jurisdiction_code, d.date_lecture
                         FROM decision_sources s JOIN decisions d ON d.id = s.decision_id
                         WHERE s.source = 'judilibre' AND s.ingested_at > now() - make_interval(days => $1)),
                     pair AS MATERIALIZED (
                         SELECT c.decision_id AS copy, c.docket_numbers, k.id AS keeper
                         FROM copy c
                         CROSS JOIN LATERAL (
                             SELECT k.id
                             FROM decisions k
                             JOIN decision_sources ks ON ks.decision_id = k.id AND ks.source = 'judilibre'
                                                     AND ks.deleted_at IS NULL
                                                     AND ks.source_fields->'sections' @> '[{{"kind":"dispositif"}}]'
                                                     AND ks.source_fields->>'chamber' IS NOT DISTINCT FROM c.chamber
                             WHERE k.jurisdiction_code = c.jurisdiction_code AND k.date_lecture = c.date_lecture
                               AND k.deleted_at IS NULL AND k.id <> c.decision_id
                             OFFSET 0) k
                         WHERE EXISTS (SELECT 1 FROM fresh f
                                       WHERE f.jurisdiction_code = c.jurisdiction_code
                                         AND f.date_lecture = c.date_lecture))
                     SELECT * FROM (
                         SELECT pr.copy, pr.docket_numbers, {copy_text} AS copy_text,
                                pr.keeper, {keeper_text} AS keeper_text
                         FROM pair pr
                         JOIN decisions cd ON cd.id = pr.copy
                         JOIN decisions kd ON kd.id = pr.keeper) t
                     WHERE t.copy_text IS NOT NULL AND t.keeper_text ~* 'jonction|\mjoint'"#,
                    copy_text = super::pick::text("p.full_text", "cd"),
                    keeper_text = super::pick::text("p.full_text", "kd"),
                ),
                &[&days],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| JoinderPair {
                copy: r.get(0),
                dockets: r.get::<_, Option<Vec<String>>>(1).unwrap_or_default(),
                copy_text: r.get(2),
                keeper: r.get(3),
                keeper_text: r.get(4),
            })
            .collect())
    }

    /// Décisions CE actives comptées par année de lecture : le côté « fonds » du
    /// recensement, à comparer à celui d'ArianeWeb.
    #[tracing::instrument(name = "db.ce_counts_by_year", skip(self), fields(db.system = "postgresql"))]
    pub async fn ce_counts_by_year(&self) -> Result<Vec<(i32, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT extract(year from date_lecture)::int, count(*) \
                 FROM decisions \
                 WHERE jurisdiction_type = 'CE' AND deleted_at IS NULL \
                   AND date_lecture IS NOT NULL \
                 GROUP BY 1",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i32>(0), r.get::<_, i64>(1)))
            .collect())
    }

    /// Checksums des bundles ArianeWeb déjà en base (`source = 'ariane-web'`), par
    /// `source_uid` — skip d'idempotence avant upsert.
    #[tracing::instrument(name = "db.ariane_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn ariane_checksums(&self) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM decision_sources \
                 WHERE source = 'ariane-web' AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Upsert d'un commentaire de norme. Ancré sur le **slug** du texte : le rendu le
    /// détient déjà, et une re-clé du `text_uid` en source ne peut pas orphaniner
    /// l'ancre. `num_key` NULL = commentaire du texte entier. Conflit sur `source_uid`
    /// (idempotent).
    #[tracing::instrument(name = "db.upsert_article_commentaire", skip(self, source_fields), fields(db.system = "postgresql"))]
    pub async fn upsert_article_commentaire(
        &self,
        text_slug: &str,
        num_key: Option<&str>,
        source: &str,
        source_uid: &str,
        content_checksum: &str,
        source_fields: &Value,
    ) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO article_commentaire
                  (text_slug, num_key, source, source_uid, content_checksum, source_fields)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (source_uid) DO UPDATE SET
                  text_slug = EXCLUDED.text_slug,
                  num_key = EXCLUDED.num_key,
                  source = EXCLUDED.source,
                  content_checksum = EXCLUDED.content_checksum,
                  source_fields = EXCLUDED.source_fields,
                  deleted_at = NULL,
                  ingested_at = NOW()
                ",
                &[
                    &text_slug,
                    &num_key,
                    &source,
                    &source_uid,
                    &content_checksum,
                    &source_fields,
                ],
            )
            .await?;
        Ok(())
    }

    /// Checksums des commentaires de norme d'une `source`, par `source_uid` — skip
    /// d'idempotence avant upsert.
    #[tracing::instrument(name = "db.article_commentaire_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_commentaire_checksums(
        &self,
        source: &str,
    ) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM article_commentaire \
                 WHERE source = $1 AND deleted_at IS NULL",
                &[&source],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Tombstone des commentaires d'une `source` **absents du jeu courant** : le
    /// dataset curé est la source de vérité, une entrée retirée doit disparaître de la
    /// fiche. Renvoie le nombre de lignes retirées.
    #[tracing::instrument(name = "db.tombstone_absent_article_commentaires", skip(self, keep), fields(db.system = "postgresql"))]
    pub async fn tombstone_absent_article_commentaires(
        &self,
        source: &str,
        keep: &[String],
    ) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE article_commentaire SET deleted_at = NOW() \
                 WHERE source = $1 AND deleted_at IS NULL \
                   AND NOT (source_uid = ANY($2))",
                &[&source, &keep],
            )
            .await?)
    }

    /// Commentaires d'un article de norme (page `/norm/{juris}/{code}/{num_key}`) : entrées
    /// `commentaires[]` ancrées sur cet article **et** celles du texte entier
    /// (`num_key IS NULL`, ex. débats de la loi). Agrégées en un tableau jsonb, `None`
    /// si aucune.
    ///
    /// Clé = le `slug` d'URL de la page, que le handler tient déjà — aucune résolution
    /// d'identité entre l'URL et cette lecture.
    #[tracing::instrument(name = "db.article_commentaires", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_commentaires(
        &self,
        text_slug: &str,
        num_key: &str,
    ) -> Result<Option<Value>> {
        let row = self
            .conn
            .query_opt(
                "SELECT jsonb_agg(c) FROM article_commentaire ac \
                 CROSS JOIN LATERAL jsonb_array_elements(ac.source_fields->'commentaires') c \
                 WHERE ac.text_slug = $1 AND (ac.num_key = $2 OR ac.num_key IS NULL) \
                   AND ac.deleted_at IS NULL \
                   AND jsonb_typeof(ac.source_fields->'commentaires') = 'array'",
                &[&text_slug, &num_key],
            )
            .await?;
        Ok(row.and_then(|r| r.get::<_, Option<Value>>(0)))
    }

    /// Slug of the law bearing each of these numbers (`2014-873`), through the
    /// slug that prefixes them. Most numbers cited in a HAL deposit's title
    /// resolve there: the rest predates the corpus or is misspelt.
    #[tracing::instrument(name = "db.law_slugs_by_numbers", skip(self, numbers), fields(db.system = "postgresql"))]
    pub async fn law_slugs_by_numbers(&self, numbers: &[String]) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT n.num, lt.slug \
                 FROM unnest($1::text[]) AS n(num) \
                 JOIN legal_text lt ON lt.nature = 'LOI' \
                   AND lt.slug LIKE 'loi-n-' || n.num || '-du-%'",
                &[&numbers],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Commentaries anchored on the whole text (`num_key IS NULL`), for a text's
    /// page. An article reading adds its own to them.
    #[tracing::instrument(name = "db.text_commentaires", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_commentaires(&self, text_slug: &str) -> Result<Option<Value>> {
        let row = self
            .conn
            .query_opt(
                "SELECT jsonb_agg(c) FROM article_commentaire ac \
                 CROSS JOIN LATERAL jsonb_array_elements(ac.source_fields->'commentaires') c \
                 WHERE ac.text_slug = $1 AND ac.num_key IS NULL \
                   AND ac.deleted_at IS NULL \
                   AND jsonb_typeof(ac.source_fields->'commentaires') = 'array'",
                &[&text_slug],
            )
            .await?;
        Ok(row.and_then(|r| r.get::<_, Option<Value>>(0)))
    }

    /// Résout une décision par (n° de dossier, date de lecture) — le n° seul n'est
    /// pas unique. Renvoie les `(id, public_id)` correspondants, plusieurs si des
    /// dossiers homonymes partagent la date.
    #[tracing::instrument(name = "db.decisions_by_docket_date", skip(self), fields(db.system = "postgresql"))]
    pub async fn decisions_by_docket_date(
        &self,
        docket: &str,
        date_iso: &str,
    ) -> Result<Vec<(i64, String)>> {
        // pg_search refuses a predicate on bare `docket_numbers`, indexed by two BM25 expressions.
        let rows = self
            .conn
            .query(
                "SELECT id, public_id FROM decisions \
                 WHERE lj_docket_keys(docket_numbers) && lj_docket_keys(ARRAY[$1::text]) \
                   AND EXISTS (SELECT 1 FROM unnest(docket_numbers) x(dk) WHERE x.dk = $1) \
                   AND date_lecture::text = $2 AND deleted_at IS NULL",
                &[&docket, &date_iso],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Cassation decisions carrying one of these docket numbers, with their
    /// date.
    ///
    /// The number alone is not unique: a number shared by several decisions of the
    /// fonds gives the link to all of them.
    #[tracing::instrument(name = "db.cassation_decisions_by_dockets", skip(self, dockets), fields(db.system = "postgresql"))]
    pub async fn cassation_decisions_by_dockets(
        &self,
        dockets: &[String],
    ) -> Result<Vec<(i64, String, Option<NaiveDate>)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.public_id, d.date_lecture \
                 FROM decisions d, unnest(d.docket_numbers) x(dk) \
                 WHERE lj_docket_keys(d.docket_numbers) && lj_docket_keys($1) \
                   AND x.dk = ANY($1) \
                   AND d.jurisdiction_type = 'CC' AND d.deleted_at IS NULL",
                &[&dockets],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Option<NaiveDate>>(2),
                )
            })
            .collect())
    }

    /// Publisher and title of every note a `source` links, by decision.
    #[tracing::instrument(name = "db.web_note_heads", skip(self), fields(db.system = "postgresql"))]
    pub async fn web_note_heads(&self, source: &str) -> Result<Vec<WebNoteHead>> {
        let rows = self
            .conn
            .query(
                "SELECT ds.decision_id, c->>'publisher', c->>'title' \
                 FROM decision_sources ds \
                 CROSS JOIN LATERAL jsonb_array_elements(ds.source_fields->'commentaires') c \
                 WHERE ds.source = $1 AND ds.deleted_at IS NULL \
                   AND jsonb_typeof(ds.source_fields->'commentaires') = 'array'",
                &[&source],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| WebNoteHead {
                decision_id: r.get(0),
                publisher: r.get(1),
                title: r.get(2),
            })
            .collect())
    }

    /// Checksums of a `source`'s web commentary bundles, by `source_uid`:
    /// idempotence skip before upsert.
    #[tracing::instrument(name = "db.web_commentary_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn web_commentary_checksums(&self, source: &str) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM decision_sources \
                 WHERE source = $1 AND deleted_at IS NULL",
                &[&source],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Tombstones a `source`'s web commentary bundles absent from `keep`. Such a
    /// bundle never carries its decision's text, so no `reconcile` follows.
    #[tracing::instrument(name = "db.tombstone_absent_web_commentaries", skip(self, keep), fields(db.system = "postgresql"))]
    pub async fn tombstone_absent_web_commentaries(
        &self,
        source: &str,
        keep: &[String],
    ) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE decision_sources SET deleted_at = NOW() \
                 WHERE source = $1 AND deleted_at IS NULL \
                   AND NOT (source_uid = ANY($2))",
                &[&source, &keep],
            )
            .await?)
    }

    /// The provenance of `source`, another than `source_uid`, that already holds `text` at this
    /// court and date under the same dockets: the record a publisher posted twice. Two decisions
    /// of one case file or of one day, each with its own text, are not twins.
    #[tracing::instrument(name = "db.source_twin", skip(self, docket_numbers, text), fields(db.system = "postgresql"))]
    pub async fn source_twin(
        &self,
        source: &str,
        source_uid: &str,
        court: &str,
        date: NaiveDate,
        docket_numbers: &[String],
        text: &str,
    ) -> Result<Option<String>> {
        let mut uids: HashMap<i64, String> = self
            .conn
            .query(
                "SELECT d.id, s.source_uid FROM decisions d \
                 JOIN decision_sources s ON s.decision_id = d.id \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_code = $1 AND d.date_lecture = $2 \
                   AND cardinality(lj_docket_keys($3)) > 0 \
                   AND lj_docket_keys(d.docket_numbers) @> lj_docket_keys($3) \
                   AND lj_docket_keys(d.docket_numbers) <@ lj_docket_keys($3) \
                   AND s.deleted_at IS NULL AND s.source = $4 AND s.source_uid <> $5",
                &[&court, &date, &docket_numbers, &source, &source_uid],
            )
            .await?
            .into_iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        let ids: Vec<i64> = uids.keys().copied().collect();
        if ids.is_empty() {
            return Ok(None);
        }
        Ok(match self.best_containment(text, &ids).await? {
            Some((c, id)) if c >= SOURCE_TWIN_CONTAINMENT => uids.remove(&id),
            _ => None,
        })
    }
}
