//! Lectures/écritures hors chemin d'upsert : format de source, résolution
//! `public_id`↔`id`, corps/texte pour les bancs offline, backfills de reprise
//! (re-extract, summary), génération et service des sitemaps.

use super::support::now;
use super::types::{
    CollageRow, DisplayInput, GtDoc, MissingSummaryRow, SitemapDecision, SitemapRow, SummaryStamp,
};
use super::DecisionRepository;
use crate::error::Result;
use chrono::{DateTime, NaiveDate, Utc};
use lj_versions::{CERTIFIER_VERSION, EXTRACT_VERSION};
use serde_json::Value;
use std::collections::HashMap;
use std::ops::Range;
use tokio_postgres::types::Json;

/// A summary as stored, its body and the court's words located in codepoints.
#[derive(Debug, Clone)]
pub struct StoredSummary {
    pub decision_id: i64,
    pub summary: String,
    pub body: Option<Range<usize>>,
    pub quote: Option<Range<usize>>,
}

/// What a summary is written from besides the decision's text: the name the decision
/// is known by, and the classification fields of each of its provenances.
#[derive(Debug, Clone)]
pub struct SummaryContext {
    pub decision_id: i64,
    pub case_name: Option<String>,
    pub provenances: Vec<(String, Value)>,
}

/// What rebuilds a decision's passages: its read text, the summary it carries
/// and the provenance that wrote it, and the other texts its passages read.
#[derive(Debug, Clone)]
pub struct EmbedInput {
    pub full_text: String,
    pub source_fields: Value,
    pub source_uid: String,
    pub lang: String,
    pub summary: Option<String>,
    pub stamp: SummaryStamp,
    pub others: Vec<EmbedText>,
}

/// A text of a decision and the provenance that wrote it.
#[derive(Debug, Clone)]
pub struct EmbedText {
    pub full_text: String,
    pub source_fields: Value,
    pub source_uid: String,
    pub lang: String,
}

/// Sort d'une écriture de révision manuelle par [`DecisionRepository::upsert_fields_at_version`].
///
/// `Unchanged` est distinct de `NotFound` : la décision existe et porte déjà
/// exactement cette révision, et l'appelant doit poursuivre son traitement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualUpsert {
    /// Au moins un champ différait : la ligne est réécrite, `updated_at` bumpé.
    Applied,
    /// Révision identique à celle en base : rien écrit, `updated_at` intact.
    Unchanged,
    /// Aucune décision pour ce `source_uid`, ou sa version l'emporte.
    NotFound,
}

/// Courts a re-extraction is scoped to.
pub enum Courts {
    /// `decisions.jurisdiction_type`, an order of courts (`CNDA`, `CEDH`).
    Types(Vec<String>),
    /// `decisions.jurisdiction_code`, named courts.
    Codes(Vec<String>),
}

/// Champs d'extraction d'une révision chargée manuellement — mêmes colonnes de
/// `decisions` que l'extraction déterministe, seule la version diffère. Elle
/// couvre TOUT ce qui est extrait — champs, facettes, citations — d'un bloc. Le
/// parsing du format d'annotation vit chez l'appelant ; ici n'arrive que la
/// projection typée.
#[derive(Debug, Clone, Default)]
pub struct ManualFields {
    pub date_lecture: Option<NaiveDate>,
    pub date_audience: Option<NaiveDate>,
    pub docket_numbers: Vec<String>,
    pub publication_codes: Vec<String>,
    pub solution_uid: Option<String>,
    pub procedure_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    /// Column `decisions.seat` (jsonb), what the annotator wrote, resolved at
    /// write time: the gold annotates a room label, and one parse turns it into
    /// both forms.
    pub seat: Option<lj_core::chamber::Seat>,
    pub formation_uid: Option<String>,
    pub jurisdiction_code: Option<String>,
}

impl DecisionRepository<'_> {
    /// Persiste le `payload_format` (`xml`/`json`) d'une décision : c'est lui qui
    /// choisit le reconstructeur côté re-extract (`fetch_reextract_inputs`).
    #[tracing::instrument(name = "db.set_payload_format", skip(self), fields(db.system = "postgresql"))]
    pub async fn set_payload_format(&self, decision_id: i64, payload_format: &str) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO decision_full_text (decision_id, payload_format)
                VALUES ($1, $2)
                ON CONFLICT (decision_id) DO UPDATE SET
                  payload_format = EXCLUDED.payload_format
                ",
                &[&decision_id, &payload_format],
            )
            .await?;
        Ok(())
    }

    /// Résout des `public_id` (base62 exposés par l'API) vers les `id`
    /// canoniques. Les ids inconnus sont simplement absents du résultat.
    #[tracing::instrument(name = "db.resolve_public_ids", skip_all, fields(db.system = "postgresql"))]
    pub async fn resolve_public_ids(&self, public_ids: &[String]) -> Result<Vec<(String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT public_id, id FROM decisions WHERE public_id = ANY($1)",
                &[&public_ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
            .collect())
    }

    /// Reading date per `decision_id`. Rows with a null `date_lecture` are
    /// omitted: the offline ranking bench needs the date to drop hits the
    /// ground truth could not have judged, and no date means no bound.
    pub async fn fetch_reading_dates(&self, ids: &[i64]) -> Result<Vec<(i64, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, date_lecture FROM decisions \
                 WHERE id = ANY($1) AND date_lecture IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, NaiveDate>(1)))
            .collect())
    }

    /// Métadonnées d'identification par `decision_id` : `(id, public_id,
    /// search_title)`. Sens inverse de [`Self::resolve_public_ids`]. Les décisions
    /// sans `public_id` sont omises.
    pub async fn fetch_public_meta(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, public_id, search_title FROM decisions \
                 WHERE id = ANY($1) AND public_id IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.try_get::<_, Option<String>>(2)
                        .unwrap_or_default()
                        .unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Résumé v4 + titre par `decision_id` : `(id, search_title, summary)`.
    /// Décisions sans `summary` omises.
    pub async fn fetch_summaries(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT decision_id, COALESCE(search_title, ''), summary FROM decision_text \
                 WHERE decision_id = ANY($1) AND {read} AND summary IS NOT NULL AND summary <> ''",
                    read = super::pick::read("decision_text")
                ),
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                )
            })
            .collect())
    }

    /// Texte intégral par `decision_id` : `(id, public_id, search_title,
    /// full_text)`, lu sur `decisions.full_text` (grain décision). Décisions sans
    /// `public_id`/`full_text` omises.
    pub async fn fetch_bodies(&self, ids: &[i64]) -> Result<Vec<(i64, String, String, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT d.id, d.public_id, COALESCE(d.search_title, ''), dt.full_text \
                 FROM decisions d JOIN decision_text dt ON dt.decision_id = d.id AND {read} \
                 WHERE d.id = ANY($1) AND d.public_id IS NOT NULL",
                    read = super::pick::read("dt")
                ),
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                    r.get::<_, String>(3),
                )
            })
            .collect())
    }

    /// Texte intégral (`decisions.full_text`, grain décision) par id. Décisions sans
    /// `full_text` omises.
    pub async fn fetch_full_texts(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT decision_id, full_text FROM decision_text \
                 WHERE decision_id = ANY($1) AND {read}",
                    read = super::pick::read("decision_text")
                ),
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Corps + métadonnées grain décision pour la matérialisation GT : `full_text`
    /// indexé + juridiction/niveau/date/titre. Décisions sans `full_text` omises.
    pub async fn fetch_gt_docs(&self, ids: &[i64]) -> Result<Vec<GtDoc>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT d.id, d.public_id, COALESCE(j.label, ''), \
                 COALESCE(d.jurisdiction_type, ''), COALESCE(d.date_lecture::text, ''), \
                 COALESCE(d.search_title, ''), dt.full_text FROM decisions d \
                 JOIN decision_text dt ON dt.decision_id = d.id AND {read} \
                 LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                 WHERE d.id = ANY($1) AND d.public_id IS NOT NULL",
                    read = super::pick::read("dt")
                ),
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| GtDoc {
                id: r.get(0),
                public_id: r.get(1),
                jurisdiction_name: r.get(2),
                jurisdiction_type: r.get(3),
                date_lecture: r.get(4),
                search_title: r.get(5),
                full_text: r.get(6),
            })
            .collect())
    }

    /// Échantillon aléatoire de `full_text` (`TABLESAMPLE SYSTEM`). `sample_pct` est
    /// piloté par le code, jamais une entrée externe.
    pub async fn sample_full_texts(&self, sample_pct: f64) -> Result<Vec<String>> {
        let sql = format!("SELECT full_text FROM decision_text TABLESAMPLE SYSTEM ({sample_pct})");
        let rows = self.conn.query(sql.as_str(), &[]).await?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    /// Fréquence documentaire `df` d'un terme indexé `term` dans `full_text`.
    /// `term` est déjà folded/lowercased comme le tokenizer de l'index ;
    /// `paradedb.term` ne le re-tokenise pas.
    pub async fn bm25_term_doc_count(&self, term: &str) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*) FROM decision_text \
                 WHERE id @@@ paradedb.term('full_text', $1::text)",
                &[&term],
            )
            .await?;
        Ok(row.get::<_, i64>(0))
    }

    #[tracing::instrument(name = "db.max_decision_id", skip(self), fields(db.system = "postgresql"))]
    pub async fn max_decision_id(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one("SELECT COALESCE(MAX(id), 0) FROM decisions", &[])
            .await?;
        Ok(row.get::<_, i64>(0))
    }

    /// Entrées du re-extract : `(payload_format, full_text, source_fields,
    /// source_uid)`, de quoi reconstruire le payload source et le re-parser. Les
    /// trois derniers viennent de la provenance autoritaire (rang max active),
    /// celle dont le texte EST `full_text`. `None` si pas de provenance active ou si
    /// `full_text`/`source_fields` manque.
    /// Décisions dont aucune provenance ne porte de charge : elles ne peuvent
    /// pas être ré-extraites, et rien dans le produit ne le dit.
    #[tracing::instrument(name = "db.decisions_sans_charge", skip(self), fields(db.system = "postgresql"))]
    pub async fn decisions_sans_charge(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.public_id
                FROM decisions d
                WHERE d.deleted_at IS NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM decision_sources ds
                    WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL
                      AND ds.source_rank > 0
                  )
                ORDER BY d.id
                ",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// A decision's re-extraction load: its most authoritative provenance. Rank 0
    /// marks a row that carries no decision (an ArianeWeb commentary bundle, a
    /// doctrine note): it cannot be read as a load, and a load source missing
    /// from the scale leaves the decision unextracted, never misextracted.
    pub async fn fetch_reextract_inputs(
        &self,
        decision_id: i64,
    ) -> Result<Option<(String, Value, String)>> {
        let row = self
            .conn
            .query_opt(
                &format!("
                SELECT {text}, ds.source_fields, ds.source_uid
                FROM decisions d
                JOIN LATERAL (
                    SELECT source_fields, source_uid
                    FROM decision_sources
                    WHERE decision_id = d.id AND deleted_at IS NULL AND source_rank > 0
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, (lang = 'eng') IS TRUE DESC, source_uid ASC
                    LIMIT 1
                ) ds ON true
                WHERE d.id = $1
                ", text = super::pick::text("p.full_text", "d")),
                &[&decision_id],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        let full_text: Option<String> = row.get(0);
        let source_fields: Option<Value> = row.get(1);
        let source_uid: String = row.get(2);
        match (full_text, source_fields) {
            (Some(ft), Some(sf)) => Ok(Some((ft, sf, source_uid))),
            _ => Ok(None),
        }
    }

    /// For a batch of ids: `(id, read text, every active provenance that carries
    /// a decision as `(source_uid, source_fields)` in authority order,
    /// case_name)`. Incomplete ids are left out; the order is not guaranteed.
    pub async fn fetch_reextract_inputs_batch(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, String, Vec<(String, Value)>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                &format!("
                SELECT d.id, {text}, d.case_name,
                       ARRAY(SELECT ds.source_uid FROM decision_sources ds
                             WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL AND ds.source_rank > 0
                               AND ds.source_fields IS NOT NULL
                             ORDER BY ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, (ds.lang = 'eng') IS TRUE DESC, ds.source_uid ASC),
                       ARRAY(SELECT ds.source_fields FROM decision_sources ds
                             WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL AND ds.source_rank > 0
                               AND ds.source_fields IS NOT NULL
                             ORDER BY ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, (ds.lang = 'eng') IS TRUE DESC, ds.source_uid ASC)
                FROM decisions d
                WHERE d.id = ANY($1)
                ", text = super::pick::text("p.full_text", "d")),
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let full_text: Option<String> = r.get(1);
                let uids: Vec<String> = r.get(3);
                let fields: Vec<Value> = r.get(4);
                let provenances: Vec<(String, Value)> = uids.into_iter().zip(fields).collect();
                match full_text {
                    Some(ft) if !provenances.is_empty() => {
                        Some((r.get::<_, i64>(0), ft, provenances, r.get(2)))
                    }
                    _ => None,
                }
            })
            .collect())
    }

    /// Même chose, adressé par `source_uid` : le triplet `(source_uid, full_text,
    /// source_fields)`, matière de
    /// [`lj_parse::Decision::from_source_fields`]. Les uids absents ou
    /// incomplets sont omis.
    pub async fn fetch_reextract_inputs_by_source_uids(
        &self,
        source_uids: &[String],
    ) -> Result<Vec<(String, String, Value)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT ds.source_uid, {text}, ds.source_fields
                FROM decision_sources ds
                JOIN decisions d ON d.id = ds.decision_id
                WHERE ds.source_uid = ANY($1)
                  AND ds.deleted_at IS NULL
                  AND d.deleted_at IS NULL
                  AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                  AND ds.source_fields IS NOT NULL
                ",
                    text = super::pick::text("p.full_text", "d")
                ),
                &[&source_uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<_, String>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Value>(2),
                )
            })
            .collect())
    }

    /// Keyset (par `decision_id`) des décisions multi-provenances tout-judilibre
    /// (≥ 2 `decision_sources` actifs, tous `source='judilibre'`), avec la liste de
    /// leurs provenances `(source_uid, payload_format, source_fields)`. Lignes
    /// triées `(decision_id, ds.id)` : l'appelant regroupe. `last_id` = dernier
    /// `decision_id` traité.
    pub async fn fetch_judilibre_multiprovenance_batch(
        &self,
        last_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Value)>> {
        let rows = self
            .conn
            .query(
                "
                WITH ids AS (
                    SELECT decision_id
                    FROM decision_sources
                    WHERE deleted_at IS NULL AND decision_id > $1
                    GROUP BY decision_id
                    HAVING count(*) > 1
                       AND count(*) = count(*) FILTER (WHERE source = 'judilibre')
                    ORDER BY decision_id
                    LIMIT $2
                )
                SELECT ds.decision_id, ds.source_uid, ds.payload_format, ds.source_fields
                FROM decision_sources ds
                JOIN ids ON ids.decision_id = ds.decision_id
                WHERE ds.deleted_at IS NULL
                ORDER BY ds.decision_id, ds.id
                ",
                &[&last_id, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                    r.get::<_, Value>(3),
                )
            })
            .collect())
    }

    /// The decisions whose passages `embed-missing` rebuilds, by id, among those
    /// with `id % shard.1 == shard.0`. By default: a text without passages and
    /// not visited at `embed_version`, a passage lacking its embedding, or
    /// passages embedded at `embed_version` without the summary their text now
    /// carries. With `stock`, also every decision visited below `embed_version`
    /// and every passage embedded behind another summary than its text's.
    pub async fn embed_queue(
        &self,
        embed_version: i16,
        stock: bool,
        shard: (i64, i64),
    ) -> Result<Vec<i64>> {
        let mut sql = format!(
            "SELECT d.id FROM decisions d
             WHERE d.id % $3 = $2 AND d.embed_version IS DISTINCT FROM $1
               AND EXISTS (SELECT 1 FROM decision_text t WHERE t.decision_id = d.id)
               AND NOT EXISTS (SELECT 1 FROM decision_chunks c WHERE c.decision_id = d.id)
             UNION
             SELECT decision_id FROM decision_chunks WHERE embedding IS NULL AND decision_id % $3 = $2
             UNION
             SELECT d.id FROM decision_text t JOIN decisions d ON d.id = t.decision_id
             WHERE t.summary IS NOT NULL AND {read} AND d.id % $3 = $2
               AND d.embed_version = $1 AND d.embed_summary_engine IS NULL
               AND EXISTS (SELECT 1 FROM decision_chunks c WHERE c.decision_id = d.id)",
            read = super::pick::read("t")
        );
        if stock {
            sql.push_str(&format!(
                "
             UNION
             SELECT d.id FROM decisions d
             WHERE d.id % $3 = $2 AND d.embed_version IS DISTINCT FROM $1
             UNION
             SELECT d.id FROM decisions d
             CROSS JOIN LATERAL (SELECT p.summary_engine, p.summary_prompt_version
                                 FROM decision_text p WHERE p.decision_id = d.id
                                 ORDER BY {order} LIMIT 1) t
             WHERE d.id % $3 = $2
               AND (d.embed_summary_engine, d.embed_summary_prompt_version)
                   IS DISTINCT FROM (t.summary_engine, t.summary_prompt_version)
               AND EXISTS (SELECT 1 FROM decision_chunks c WHERE c.decision_id = d.id)",
                order = super::pick::order("p", "d")
            ));
        }
        sql.push_str(" ORDER BY 1");
        let rows = self
            .conn
            .query(sql.as_str(), &[&embed_version, &shard.0, &shard.1])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// What rebuilds the passages of each of `ids`, in order: `None` without a
    /// read text or without the active provenance that wrote it. Its other texts
    /// come with it when their provenance is active.
    pub async fn embed_inputs(&self, ids: &[i64]) -> Result<Vec<(i64, Option<EmbedInput>)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT ids.id, t.full_text, ds.source_fields, t.source_uid, t.summary,
                       t.summary_engine, t.summary_prompt_version, t.lang
                FROM unnest($1::bigint[]) AS ids(id)
                LEFT JOIN decisions d ON d.id = ids.id
                LEFT JOIN LATERAL (
                    SELECT p.full_text, p.source_uid, p.summary, p.summary_engine,
                           p.summary_prompt_version, p.lang
                    FROM decision_text p WHERE p.decision_id = d.id
                    ORDER BY {order} LIMIT 1
                ) t ON true
                LEFT JOIN decision_sources ds
                       ON ds.source_uid = t.source_uid AND ds.deleted_at IS NULL
                ORDER BY ids.id
                ",
                    order = super::pick::order("p", "d")
                ),
                &[&ids],
            )
            .await?;
        let read: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get::<_, Option<String>>(3))
            .collect();
        let mut others: HashMap<i64, Vec<EmbedText>> = HashMap::new();
        for r in self
            .conn
            .query(
                &format!(
                    "SELECT t.decision_id, t.full_text, ds.source_fields, t.source_uid, t.lang
                     FROM decision_text t
                     JOIN decision_sources ds ON ds.source_uid = t.source_uid AND ds.deleted_at IS NULL
                     WHERE t.decision_id = ANY($1) AND t.source_uid <> ALL($2) AND {passages}
                     ORDER BY t.decision_id, t.lang",
                    passages = super::pick::passages("t")
                ),
                &[&ids, &read],
            )
            .await?
        {
            others.entry(r.get(0)).or_default().push(EmbedText {
                full_text: r.get(1),
                source_fields: r.get(2),
                source_uid: r.get(3),
                lang: r.get(4),
            });
        }
        Ok(rows
            .iter()
            .map(|r| {
                let id: i64 = r.get(0);
                let input = match (r.get(1), r.get(2), r.get(3), r.get(7)) {
                    (Some(full_text), Some(source_fields), Some(source_uid), Some(lang)) => {
                        Some(EmbedInput {
                            full_text,
                            source_fields,
                            source_uid,
                            lang,
                            summary: r.get(4),
                            stamp: (r.get(5), r.get(6)),
                            others: others.remove(&id).unwrap_or_default(),
                        })
                    }
                    _ => None,
                };
                (id, input)
            })
            .collect())
    }

    /// Keyset des décisions à re-parser : `full_text` présent, ``extract_version``
    /// sous la version courante. Les révisions manuelles (version > courante) sont
    /// hors worklist.
    pub async fn decision_ids_for_reextract(&self, last_id: i64, limit: i64) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                  AND (d.extract_version IS NULL OR d.extract_version < $3)
                  AND (d.certified_version IS NULL OR d.certified_version < $4)
                ORDER BY d.id
                LIMIT $2
                ",
                &[&last_id, &limit, &EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Certifie un lot de décisions à `certified_version` : le re-extract par défaut
    /// les skippe tant que `certified_version >= CERTIFIER_VERSION`. Setter
    /// uniquement — l'invalidation est un bump de la constante.
    pub async fn certify_decisions(&self, ids: &[i64], version: i32) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "UPDATE decisions SET certified_version = $2 WHERE id = ANY($1)",
                &[&ids, &version],
            )
            .await?;
        Ok(n)
    }

    /// Nombre de décisions périmées, même prédicat que
    /// [`Self::decision_ids_for_reextract`] : sert l'avancement `processed/total`.
    pub async fn count_decisions_for_reextract(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "
                SELECT count(*) FROM decisions d
                WHERE EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                  AND (extract_version IS NULL OR extract_version < $1)
                  AND (certified_version IS NULL OR certified_version < $2)
                ",
                &[&EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(row.get(0))
    }

    /// Worklist complète des ids périmés, triée par id, en un seul scan, ensuite
    /// partitionnée entre workers concurrents.
    pub async fn stale_decision_ids_for_reextract(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT id FROM decisions d
                WHERE EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                  AND (extract_version IS NULL OR extract_version < $1)
                  AND (certified_version IS NULL OR certified_version < $2)
                ORDER BY id
                ",
                &[&EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Worklist de la passe intégrale (`--full`) : tout le fonds jusqu'à la version
    /// courante incluse, même déjà à jour, même certifié — c'est le relink
    /// hebdomadaire. Les révisions manuelles restent hors worklist.
    pub async fn all_decision_ids_for_reextract(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT id FROM decisions d
                WHERE EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                  AND (extract_version IS NULL OR extract_version <= $1)
                ORDER BY id
                ",
                &[&EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Écrit une révision des champs d'extraction dans les colonnes de `decisions`,
    /// stampée `extract_version = version` ; une version > courante n'est jamais
    /// réécrite par le recognizer. Les champs `None` écrivent NULL (révision
    /// exhaustive : absent = confirmé absent), sauf `jurisdiction_code` (COALESCE :
    /// porté aussi par les métadonnées source).
    ///
    /// N'écrit que sur différence : une révision identique laisse `updated_at`
    /// intact. `cible` vide = `NotFound` (pas de décision, ou version supérieure) ;
    /// `cible` non vide sans ligne écrite = `Unchanged`.
    ///
    /// `seat_paths` is NOT written here: a path resolves against the room
    /// vocabulary of the whole corpus, which only `resolve-seats` holds. So a
    /// gold load leaves the axis to that pass, as the first write of the seats
    /// did.
    pub async fn upsert_fields_at_version(
        &self,
        source_uid: &str,
        version: i16,
        fields: &ManualFields,
    ) -> Result<ManualUpsert> {
        let seat = fields.seat.clone().map(Json);
        let row = self
            .conn
            .query_one(
                "WITH cible AS (
                     SELECT d.id
                     FROM decisions d
                     JOIN decision_sources ds ON ds.decision_id = d.id
                     WHERE ds.source_uid = $1
                       AND (d.extract_version IS NULL OR d.extract_version <= $12)
                 ), maj AS (
                     UPDATE decisions d SET
                         date_lecture = $2, date_audience = $3, docket_numbers = $4,
                         publication_codes = $5,
                         solution_uid = $6, procedure_uid = $7, office_uid = $8,
                         legal_domain_uid = $9,
                         seat = $13, formation_uid = $10,
                         jurisdiction_code = COALESCE($11, d.jurisdiction_code),
                         extract_version = $12, updated_at = now()
                     FROM cible
                     WHERE d.id = cible.id
                       AND (d.date_lecture, d.date_audience, d.docket_numbers,
                            d.publication_codes, d.solution_uid, d.procedure_uid,
                            d.office_uid, d.legal_domain_uid,
                            d.seat, d.formation_uid,
                            d.jurisdiction_code, d.extract_version)
                           IS DISTINCT FROM
                           ($2, $3, $4, $5, $6, $7, $8, $9, $13, $10,
                            COALESCE($11, d.jurisdiction_code), $12)
                     RETURNING 1
                 )
                 SELECT (SELECT count(*) FROM cible), (SELECT count(*) FROM maj)",
                &[
                    &source_uid,
                    &fields.date_lecture,
                    &fields.date_audience,
                    &fields.docket_numbers,
                    &fields.publication_codes,
                    &fields.solution_uid,
                    &fields.procedure_uid,
                    &fields.office_uid,
                    &fields.legal_domain_uid,
                    &fields.formation_uid,
                    &fields.jurisdiction_code,
                    &version,
                    &seat,
                ],
            )
            .await?;
        let trouvee: i64 = row.get(0);
        let ecrite: i64 = row.get(1);
        Ok(match (trouvee, ecrite) {
            (0, _) => ManualUpsert::NotFound,
            (_, 0) => ManualUpsert::Unchanged,
            _ => ManualUpsert::Applied,
        })
    }

    /// Remplace la couche citations d'une décision à une `extract_version`
    /// explicite : le set fourni EST la vérité, DELETE + COPY via l'écrivain commun.
    /// Les `rows` arrivent déjà validées par l'appelant. `None` si aucun
    /// `source_uid` ne matche ou si la décision porte une version supérieure, sinon
    /// le nombre de lignes insérées.
    pub async fn replace_citations_at_version(
        &self,
        source_uid: &str,
        version: i16,
        rows: &[super::types::CitationOccurrenceRow],
    ) -> Result<Option<u64>> {
        let row = self
            .conn
            .query_opt(
                "SELECT ds.decision_id FROM decision_sources ds
                 JOIN decisions d ON d.id = ds.decision_id
                 WHERE ds.source_uid = $1
                   AND (d.extract_version IS NULL OR d.extract_version <= $2)",
                &[&source_uid, &version],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        let decision_id: i64 = row.get(0);
        self.write_citation_occurrences(&[(decision_id, rows)], version)
            .await?;
        Ok(Some(rows.len() as u64))
    }

    /// Keyset of the decisions of `courts` to re-parse, whatever their
    /// `extract_version` and without a global bump; manual revisions excluded.
    /// `text_query` narrows it to the decisions matching a BM25 expression: a
    /// new grammar only reaches the texts carrying the literal it needs, rather
    /// than the whole fund.
    pub async fn decision_ids_for_reextract_by_juridiction(
        &self,
        last_id: i64,
        limit: i64,
        courts: &Courts,
        text_query: Option<&str>,
    ) -> Result<Vec<i64>> {
        let filtre = match text_query {
            Some(_) => {
                "AND d.id IN (SELECT t.decision_id FROM decision_text t \
                        WHERE t.id @@@ paradedb.parse_with_field('full_text', $5))"
            }
            None => "",
        };
        let (court, names) = match courts {
            Courts::Types(names) => ("d.jurisdiction_type", names),
            Courts::Codes(names) => ("d.jurisdiction_code", names),
        };
        let sql = format!(
            "SELECT d.id
             FROM decisions d
             WHERE d.id > $1
               AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
               AND {court} = ANY($3)
               AND (d.extract_version IS NULL OR d.extract_version <= $4)
               {filtre}
             ORDER BY d.id
             LIMIT $2"
        );
        let rows = match text_query {
            Some(q) => {
                let q = q.to_string();
                self.conn
                    .query(
                        sql.as_str(),
                        &[&last_id, &limit, names, &EXTRACT_VERSION, &q],
                    )
                    .await?
            }
            None => {
                self.conn
                    .query(sql.as_str(), &[&last_id, &limit, names, &EXTRACT_VERSION])
                    .await?
            }
        };
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Décisions à re-parser ciblées par un texte cité (`ref_text_uid`),
    /// indépendamment de `extract_version` : à combiner avec
    /// `--field legal_references`. Un seul scan, GIN-piloté : la work-list part de
    /// `legal_citation` (`lj_cit_terms`), pas d'un keyset sur `decisions`.
    /// L'appelant lève le `statement_timeout`.
    pub async fn decision_ids_for_reextract_by_citing_ref_uid(
        &self,
        ref_text_uid: &str,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM (SELECT DISTINCT lc.decision_id
                      FROM legal_citation lc
                      WHERE public.lj_cit_terms(lc.spans) @> ARRAY[$1]) c
                JOIN decisions d ON d.id = c.decision_id
                WHERE EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                  AND (d.extract_version IS NULL OR d.extract_version <= $2)
                ORDER BY d.id
                ",
                &[&ref_text_uid, &EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The rows that carry a summary, over a range of the primary key.
    #[tracing::instrument(name = "db.summaries_in_range", skip(self), fields(db.system = "postgresql"))]
    pub async fn summaries_in_range(&self, from_id: i64, to_id: i64) -> Result<Vec<StoredSummary>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT decision_id, summary, summary_body_start, summary_body_end, \
                        summary_quote_start, summary_quote_end \
                 FROM decision_text \
                 WHERE decision_id >= $1 AND decision_id < $2 AND {read} \
                   AND summary IS NOT NULL AND deleted_at IS NULL",
                    read = super::pick::read("decision_text")
                ),
                &[&from_id, &to_id],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| StoredSummary {
                decision_id: r.get(0),
                summary: r.get(1),
                body: codepoints(r.get(2), r.get(3)),
                quote: codepoints(r.get(4), r.get(5)),
            })
            .collect())
    }

    /// The [`SummaryContext`] of decisions named by id, each provenance of `sources`
    /// cut to the `keys` it carries.
    #[tracing::instrument(name = "db.summary_contexts", skip(self, ids, sources, keys), fields(db.system = "postgresql", n = ids.len()))]
    pub async fn summary_contexts(
        &self,
        ids: &[i64],
        sources: &[&str],
        keys: &[&str],
    ) -> Result<Vec<SummaryContext>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.case_name, \
                        ARRAY(SELECT jsonb_build_array(ds.source, \
                                  (SELECT coalesce(jsonb_object_agg(k, ds.source_fields->k), '{}') \
                                   FROM unnest($3::text[]) k WHERE ds.source_fields ? k)) \
                              FROM decision_sources ds \
                              WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL \
                                AND ds.source = ANY($2) AND ds.source_fields IS NOT NULL \
                              ORDER BY ds.source_rank, ds.source_uid) \
                 FROM decisions d WHERE d.id = ANY($1)",
                &[&ids, &sources, &keys],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let provenances: Vec<Value> = r.get(2);
                SummaryContext {
                    decision_id: r.get(0),
                    case_name: r.get(1),
                    provenances: provenances
                        .into_iter()
                        .filter_map(|pair| match pair {
                            Value::Array(mut pair) if pair.len() == 2 => {
                                let fields = pair.pop()?;
                                Some((pair.pop()?.as_str()?.to_string(), fields))
                            }
                            _ => None,
                        })
                        .collect(),
                }
            })
            .collect())
    }

    /// `(id, commentary, analysis)` of the collages written by `collage_codes` over a
    /// range of the primary key, the commentary picked as [`Self::fetch_collage_rows`] does.
    #[tracing::instrument(name = "db.collage_commentaries_in_range", skip(self), fields(db.system = "postgresql"))]
    pub async fn collage_commentaries_in_range(
        &self,
        from_id: i64,
        to_id: i64,
        collage_codes: &[i16],
        prose_min: i32,
    ) -> Result<Vec<(i64, String, bool)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT t.decision_id, c.commentary, c.analysis
                FROM decision_text t
                JOIN LATERAL (
                    SELECT ds.source_fields->>'summary' AS commentary, false AS analysis
                    FROM decision_sources ds
                    WHERE ds.decision_id = t.decision_id AND ds.deleted_at IS NULL
                      AND ds.source = 'judilibre'
                      AND length(ds.source_fields->>'summary') > $4
                      AND (length(ds.source_fields->>'summary')
                           - length(replace(ds.source_fields->>'summary', ' - ', ''))) / 3 < 2
                    UNION ALL
                    SELECT x.v->>'body', true
                    FROM decision_sources ds,
                         LATERAL jsonb_array_elements(ds.source_fields->'commentaires') x(v)
                    WHERE ds.decision_id = t.decision_id AND ds.deleted_at IS NULL
                      AND ds.source = 'ariane-web'
                      AND x.v->>'kind' = 'analyse'
                      AND position(E'\n\n' in x.v->>'body') > 0
                      AND length(trim(substr(x.v->>'body',
                            position(E'\n\n' in x.v->>'body') + 2))) > $4
                    ORDER BY 1 DESC
                    LIMIT 1
                ) c ON true
                WHERE t.decision_id >= $1 AND t.decision_id < $2 AND {read}
                  AND t.deleted_at IS NULL AND t.summary_engine = ANY($3)
                ",
                    read = super::pick::read("t")
                ),
                &[&from_id, &to_id, &collage_codes, &prose_min],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Replaces the text alone, where [`Self::set_summary`] also stamps the engine
    /// and the generation: repairing the stock after the deterministic cleaning of
    /// `lj-core` gains a rule does not change who wrote the row.
    #[tracing::instrument(name = "db.rewrite_summary_text", skip(self, summary), fields(db.system = "postgresql"))]
    pub async fn rewrite_summary_text(
        &self,
        decision_id: i64,
        summary: &str,
        body: Option<Range<usize>>,
        quote: Option<Range<usize>>,
    ) -> Result<()> {
        let (body_start, body_end) = codepoint_columns(body);
        let (start, end) = codepoint_columns(quote);
        self.conn
            .execute(
                &format!(
                    "WITH t AS (UPDATE decision_text SET summary = $1, summary_quote_start = $4, \
                            summary_quote_end = $5, summary_body_start = $6, summary_body_end = $7 \
                            WHERE decision_id = $3 AND {read} RETURNING decision_id) \
                 UPDATE decisions SET updated_at = $2 WHERE id = (SELECT decision_id FROM t)",
                    read = super::pick::read("decision_text")
                ),
                &[
                    &summary,
                    &now(),
                    &decision_id,
                    &start,
                    &end,
                    &body_start,
                    &body_end,
                ],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(name = "db.set_summary", skip(self, summary), fields(db.system = "postgresql"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn set_summary(
        &self,
        decision_id: i64,
        summary: &str,
        body: Option<Range<usize>>,
        quote: Option<Range<usize>>,
        prompt_version: i16,
        engine: i16,
    ) -> Result<()> {
        let (body_start, body_end) = codepoint_columns(body);
        let (start, end) = codepoint_columns(quote);
        self.conn
            .execute(
                &format!("WITH t AS (UPDATE decision_text SET summary = $1, summary_prompt_version = $2, \
                            summary_engine = $3, summary_quote_start = $6, summary_quote_end = $7, \
                            summary_body_start = $8, summary_body_end = $9 \
                            WHERE decision_id = $5 AND {read} RETURNING decision_id) \
                 UPDATE decisions SET updated_at = $4 WHERE id = (SELECT decision_id FROM t)", read = super::pick::read("decision_text")),
                &[
                    &summary,
                    &prompt_version,
                    &engine,
                    &now(),
                    &decision_id,
                    &start,
                    &end,
                    &body_start,
                    &body_end,
                ],
            )
            .await?;
        Ok(())
    }

    /// [`Self::set_summary`] on a row still below `target_version`, false when another
    /// writer reached the generation first.
    #[tracing::instrument(name = "db.set_stale_summary", skip(self, summary), fields(db.system = "postgresql"))]
    pub async fn set_stale_summary(
        &self,
        decision_id: i64,
        summary: &str,
        body: Option<Range<usize>>,
        target_version: i16,
        engine: i16,
    ) -> Result<bool> {
        let (body_start, body_end) = codepoint_columns(body);
        let written = self
            .conn
            .execute(
                &format!("WITH t AS (UPDATE decision_text SET summary = $1, summary_prompt_version = $2, \
                            summary_engine = $3, summary_quote_start = NULL, summary_quote_end = NULL, \
                            summary_body_start = $6, summary_body_end = $7 \
                            WHERE decision_id = $5 AND {read} \
                              AND (summary IS NULL OR summary_prompt_version IS NULL \
                                   OR summary_prompt_version < $2) \
                            RETURNING decision_id) \
                 UPDATE decisions SET updated_at = $4 WHERE id = (SELECT decision_id FROM t)", read = super::pick::read("decision_text")),
                &[
                    &summary,
                    &target_version,
                    &engine,
                    &now(),
                    &decision_id,
                    &body_start,
                    &body_end,
                ],
            )
            .await?;
        Ok(written == 1)
    }

    /// The rows [`Self::iter_summaries_by_jurisdiction`] yields, for decisions named by id.
    #[tracing::instrument(name = "db.summary_rows", skip(self, ids), fields(db.system = "postgresql", n = ids.len()))]
    pub async fn summary_rows(&self, ids: &[i64]) -> Result<Vec<MissingSummaryRow>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.public_id, d.jurisdiction_type, j.label, \
                        d.date_lecture::text, d.docket_numbers \
                 FROM decisions d \
                 LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                 WHERE d.id = ANY($1) AND d.public_id IS NOT NULL AND d.deleted_at IS NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| {
                let docket: Option<Vec<String>> = row.get(5);
                MissingSummaryRow {
                    decision_id: row.get(0),
                    public_id: row.get(1),
                    jurisdiction_type: row.get(2),
                    jurisdiction_name: row.get(3),
                    date_lecture: row.get(4),
                    docket_numbers: docket.filter(|v| !v.is_empty()),
                }
            })
            .collect())
    }

    /// Iterates over every active decision, in PK order. `lastmod` =
    /// `GREATEST(date_lecture, updated_at::date)`. Decisions without a `public_id`
    /// are excluded; keyset pagination on `d.id`, materialised as a `Vec`.
    #[tracing::instrument(name = "db.iter_decisions_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_decisions_for_sitemap(
        &self,
        batch_size: i64,
    ) -> Result<Vec<SitemapDecision>> {
        let mut out: Vec<SitemapDecision> = Vec::new();
        let mut last_id: i64 = 0;
        loop {
            let rows = self
                .conn
                .query(
                    "
                SELECT id, public_id,
                       GREATEST(date_lecture, updated_at::date) AS lastmod,
                       jurisdiction_type, jurisdiction_code
                FROM decisions
                WHERE public_id IS NOT NULL
                  AND deleted_at IS NULL
                  AND id > $1
                ORDER BY id
                LIMIT $2
                ",
                    &[&last_id, &batch_size],
                )
                .await?;
            if rows.is_empty() {
                return Ok(out);
            }
            for row in &rows {
                let public_id: String = row.get(1);
                let lastmod: Option<NaiveDate> = row.get(2);
                out.push((public_id, row.get(3), row.get(4), lastmod));
            }
            last_id = rows[rows.len() - 1].get::<_, i64>(0);
            if (rows.len() as i64) < batch_size {
                return Ok(out);
            }
        }
    }

    /// Remplace l'intégralité de la table `sitemaps` par `files` : les sub-sitemaps
    /// d'un run précédent disparaissent. L'appelant wrappe l'appel dans une
    /// transaction.
    #[tracing::instrument(name = "db.replace_sitemaps", skip(self, files), fields(db.system = "postgresql", count = files.len()))]
    pub async fn replace_sitemaps(&self, files: &[SitemapRow]) -> Result<()> {
        self.conn.execute("DELETE FROM sitemaps", &[]).await?;
        for f in files {
            self.conn
                .execute(
                    "INSERT INTO sitemaps (filename, content_type, body, lastmod) \
                     VALUES ($1, $2, $3, $4)",
                    &[&f.filename, &f.content_type, &f.body, &f.lastmod],
                )
                .await?;
        }
        Ok(())
    }

    /// Lit un sitemap par nom de fichier — `(body, content_type)`, `None` si absent.
    #[tracing::instrument(name = "db.fetch_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn fetch_sitemap(&self, filename: &str) -> Result<Option<(Vec<u8>, String)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT body, content_type FROM sitemaps WHERE filename = $1",
                &[&filename],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    /// `(public_id, jurisdiction_type, jurisdiction_code)` of the decisions whose provenance was
    /// (re)ingested since `since`, freshest first.
    ///
    /// `ingested_at` only moves on a new provenance, a `content_checksum` change or
    /// a tombstone lift, where `decisions.updated_at` follows every maintenance pass.
    #[tracing::instrument(name = "db.iter_public_ids_ingested_since", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_public_ids_ingested_since(
        &self,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT public_id, jurisdiction_type, jurisdiction_code FROM (
                    SELECT DISTINCT ON (d.public_id)
                           d.public_id AS public_id, d.jurisdiction_type AS jurisdiction_type,
                           d.jurisdiction_code AS jurisdiction_code,
                           s.ingested_at AS ingested_at
                    FROM decision_sources s
                    JOIN decisions d ON d.id = s.decision_id
                    WHERE s.ingested_at >= $1
                      AND s.deleted_at IS NULL
                      AND d.deleted_at IS NULL
                      AND d.public_id IS NOT NULL
                    ORDER BY d.public_id, s.ingested_at DESC
                ) f
                ORDER BY f.ingested_at DESC
                LIMIT $2
                ",
                &[&since, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Every decision a collage engine can serve and none has written, in one pass.
    ///
    /// Driven from `decision_sources`, which is the selective side: few of its
    /// rows carry a commentary, so probing `decisions` one by one costs a scan of
    /// the whole table.
    ///
    /// The usable-commentary test is structural and differs by source, a length bar
    /// admitting nomenclature chains of 200 to 400 characters: a Cour de cassation
    /// summary is prose from its first word, so it is taken when it is not dense in
    /// ` - `, while a Conseil d'Etat analysis opens on an indexation chain, so it is
    /// taken only when a written body follows a blank line.
    #[tracing::instrument(name = "db.collage_queue", skip(self), fields(db.system = "postgresql"))]
    pub async fn collage_queue(&self, collage_codes: &[i16], prose_min: i32) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "
                WITH porteuses AS (
                    SELECT ds.decision_id
                    FROM decision_sources ds
                    WHERE ds.deleted_at IS NULL AND ds.source = 'judilibre'
                      AND length(ds.source_fields->>'summary') > $2
                      AND (length(ds.source_fields->>'summary')
                           - length(replace(ds.source_fields->>'summary', ' - ', ''))) / 3 < 2
                    UNION
                    SELECT ds.decision_id
                    FROM decision_sources ds,
                         LATERAL jsonb_array_elements(ds.source_fields->'commentaires') x(v)
                    WHERE ds.deleted_at IS NULL AND ds.source = 'ariane-web'
                      AND x.v->>'kind' = 'analyse'
                      AND position(E'\n\n' in x.v->>'body') > 0
                      AND length(trim(substr(x.v->>'body',
                            position(E'\n\n' in x.v->>'body') + 2))) > $2
                )
                SELECT t.decision_id
                FROM porteuses p
                JOIN decision_text t ON t.decision_id = p.decision_id AND {read}
                WHERE t.deleted_at IS NULL
                  AND (t.summary_engine IS NULL OR t.summary_engine <> ALL($1))
                ORDER BY t.decision_id
                ",
                    read = super::pick::read("t")
                ),
                &[&collage_codes, &prose_min],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The body and the commentary of a chunk of the queue.
    ///
    /// Longest commentary wins when a decision carries several, which is what the
    /// bench graded.
    #[tracing::instrument(name = "db.fetch_collage_rows", skip(self, ids), fields(db.system = "postgresql"))]
    pub async fn fetch_collage_rows(&self, ids: &[i64], prose_min: i32) -> Result<Vec<CollageRow>> {
        let rows = self
            .conn
            .query(
                &format!("
                SELECT d.id, {text}, c.commentary, c.analysis
                FROM decisions d
                JOIN LATERAL (
                    SELECT ds.source_fields->>'summary' AS commentary, false AS analysis
                    FROM decision_sources ds
                    WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL
                      AND ds.source = 'judilibre'
                      AND length(ds.source_fields->>'summary') > $2
                      AND (length(ds.source_fields->>'summary')
                           - length(replace(ds.source_fields->>'summary', ' - ', ''))) / 3 < 2
                    UNION ALL
                    SELECT x.v->>'body', true
                    FROM decision_sources ds,
                         LATERAL jsonb_array_elements(ds.source_fields->'commentaires') x(v)
                    WHERE ds.decision_id = d.id AND ds.deleted_at IS NULL
                      AND ds.source = 'ariane-web'
                      AND x.v->>'kind' = 'analyse'
                      AND position(E'\n\n' in x.v->>'body') > 0
                      AND length(trim(substr(x.v->>'body',
                            position(E'\n\n' in x.v->>'body') + 2))) > $2
                    ORDER BY 1 DESC
                    LIMIT 1
                ) c ON true
                WHERE d.id = ANY($1) AND EXISTS (SELECT 1 FROM decision_text dt WHERE dt.decision_id = d.id)
                ORDER BY d.id
                ", text = super::pick::text("p.full_text", "d")),
                &[&ids, &prose_min],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CollageRow {
                decision_id: r.get(0),
                full_text: r.get(1),
                commentary: r.get(2),
                analysis: r.get(3),
            })
            .collect())
    }

    /// Itère les décisions sans summary à jour, par batches.
    ///
    /// Chaque ligne = `(decision_id, public_id, jurisdiction_type,
    /// jurisdiction_name, date_lecture, docket_numbers)` — les quatre dernières
    /// reconstruisent le titre côté appelant. `summary IS NULL`, plus avec
    /// `include_stale` les `summary_prompt_version < target_version`. Tombstones et
    /// décisions sans corps exclues. `limit` borne le total yieldé sur toute
    /// l'itération ; `wrap` reprend sur `]0, start_id]` une fois l'arc
    /// `]start_id, max]` épuisé.
    ///
    /// Renvoie tous les batches matérialisés : les frontières de batch, donc
    /// l'identité des appels Mistral concurrents, sont préservées.
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "db.iter_decisions_missing_summary", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_decisions_missing_summary(
        &self,
        target_version: i16,
        batch_size: i64,
        limit: Option<i64>,
        start_id: i64,
        wrap: bool,
        include_stale: bool,
    ) -> Result<Vec<Vec<MissingSummaryRow>>> {
        // Plafond sur l'arc courant : max bigint avant un éventuel wrap, puis
        // `start_id` une fois qu'on a bouclé sur le bas de la plage.
        let mut ceiling: i64 = i64::MAX;
        let mut wrapped = false;
        let mut emitted: i64 = 0;
        let mut last_id = start_id;
        let mut batches: Vec<Vec<MissingSummaryRow>> = Vec::new();
        loop {
            let remaining = match limit {
                None => batch_size,
                Some(lim) => batch_size.min(lim - emitted),
            };
            if remaining <= 0 {
                break;
            }
            let rows = self
                .conn
                .query(
                    &format!(
                        "
                SELECT d.id, d.public_id, d.jurisdiction_type, j.label,
                       d.date_lecture::text, d.docket_numbers
                FROM decisions d
                JOIN decision_text t ON t.decision_id = d.id AND {read}
                LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                WHERE d.id > $1
                  AND d.id <= $2
                  AND (
                    t.summary IS NULL
                    OR ($5 AND (
                      t.summary_prompt_version IS NULL
                      OR t.summary_prompt_version < $3
                    ))
                  )
                  AND d.public_id IS NOT NULL
                  AND d.deleted_at IS NULL
                ORDER BY d.id
                LIMIT $4
                ",
                        read = super::pick::read("t")
                    ),
                    &[
                        &last_id,
                        &ceiling,
                        &target_version,
                        &remaining,
                        &include_stale,
                    ],
                )
                .await?;
            let n = rows.len();
            if n > 0 {
                let mut batch: Vec<MissingSummaryRow> = Vec::with_capacity(n);
                for row in &rows {
                    let docket: Option<Vec<String>> = row.get(5);
                    batch.push(MissingSummaryRow {
                        decision_id: row.get(0),
                        public_id: row.get(1),
                        jurisdiction_type: row.get(2),
                        jurisdiction_name: row.get(3),
                        date_lecture: row.get(4),
                        docket_numbers: docket.filter(|v| !v.is_empty()),
                    });
                }
                emitted += n as i64;
                last_id = rows[n - 1].get::<_, i64>(0);
                batches.push(batch);
            }
            if n == 0 || (n as i64) < remaining {
                if wrap && !wrapped && start_id > 0 {
                    wrapped = true;
                    ceiling = start_id;
                    last_id = 0;
                    continue;
                }
                break;
            }
        }
        Ok(batches)
    }

    /// The same rows, most cited inside the corpus first, in the types of `only`
    /// when it names any, and only the decisions with a usage name when `named`.
    ///
    /// No keyset: `case_citation` bounds the set, so the whole order fits one
    /// query, and a rank offers nothing stable to page against.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    #[tracing::instrument(name = "db.iter_summaries_by_citations", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_summaries_by_citations(
        &self,
        target_version: i16,
        batch_size: i64,
        limit: Option<i64>,
        include_stale: bool,
        min_citations: i32,
        only: &[&str],
        named: bool,
    ) -> Result<Vec<Vec<MissingSummaryRow>>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "
                WITH cites AS (
                    SELECT target_decision_id AS id, count(*)::int AS n
                    FROM case_citation
                    WHERE target_decision_id IS NOT NULL
                    GROUP BY 1
                )
                SELECT d.id, d.public_id, d.jurisdiction_type, j.label,
                       d.date_lecture::text, d.docket_numbers
                FROM cites c
                JOIN decisions d ON d.id = c.id
                JOIN decision_text t ON t.decision_id = d.id AND {read}
                LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                WHERE c.n >= $4
                  AND (cardinality($5::text[]) = 0 OR d.jurisdiction_type = ANY($5))
                  AND (NOT $6 OR d.case_name IS NOT NULL)
                  AND (
                    t.summary IS NULL
                    OR ($2 AND (
                      t.summary_prompt_version IS NULL
                      OR t.summary_prompt_version < $1
                    ))
                  )
                  AND d.public_id IS NOT NULL
                  AND d.deleted_at IS NULL
                ORDER BY c.n DESC, d.id
                LIMIT $3
                ",
                    read = super::pick::read("t")
                ),
                &[
                    &target_version,
                    &include_stale,
                    &limit.unwrap_or(i64::MAX),
                    &min_citations,
                    &only,
                    &named,
                ],
            )
            .await?;
        Ok(rows
            .chunks(usize::try_from(batch_size.max(1)).unwrap_or(usize::MAX))
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|row| {
                        let docket: Option<Vec<String>> = row.get(5);
                        MissingSummaryRow {
                            decision_id: row.get(0),
                            public_id: row.get(1),
                            jurisdiction_type: row.get(2),
                            jurisdiction_name: row.get(3),
                            date_lecture: row.get(4),
                            docket_numbers: docket.filter(|v| !v.is_empty()),
                        }
                    })
                    .collect()
            })
            .collect())
    }

    /// Stale or missing summaries by jurisdiction tier, then most recent first, or
    /// the whole order backwards when `reverse`, in the types of `only` when it
    /// names any, and only the decisions with a usage name when `named`. A type absent from `types` comes after every tier. One query bounded by `limit`: the top-N sort holds the
    /// batch, never every candidate.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(name = "db.iter_summaries_by_jurisdiction", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_summaries_by_jurisdiction(
        &self,
        target_version: i16,
        batch_size: i64,
        limit: Option<i64>,
        include_stale: bool,
        types: &[&str],
        tiers: &[i32],
        reverse: bool,
        only: &[&str],
        named: bool,
    ) -> Result<Vec<Vec<MissingSummaryRow>>> {
        let (tier, recent) = if reverse {
            ("DESC", "ASC NULLS FIRST")
        } else {
            ("ASC", "DESC NULLS LAST")
        };
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT d.id, d.public_id, d.jurisdiction_type, j.label,
                       d.date_lecture::text, d.docket_numbers
                FROM decisions d
                JOIN decision_text t ON t.decision_id = d.id AND {read}
                LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                WHERE (
                    t.summary IS NULL
                    OR ($2 AND (
                      t.summary_prompt_version IS NULL
                      OR t.summary_prompt_version < $1
                    ))
                  )
                  AND d.public_id IS NOT NULL
                  AND d.deleted_at IS NULL
                  AND (cardinality($6::text[]) = 0 OR d.jurisdiction_type = ANY($6))
                  AND (NOT $7 OR d.case_name IS NOT NULL)
                ORDER BY coalesce(($5::int[])[array_position($4::text[], d.jurisdiction_type)], 2147483647) {tier},
                         d.date_lecture {recent}, d.id {recent_id}
                LIMIT $3
                ",
                    recent_id = if reverse { "ASC" } else { "DESC" }, read = super::pick::read("t")
                ),
                &[
                    &target_version,
                    &include_stale,
                    &limit.unwrap_or(i64::MAX),
                    &types,
                    &tiers,
                    &only,
                    &named,
                ],
            )
            .await?;
        Ok(rows
            .chunks(usize::try_from(batch_size.max(1)).unwrap_or(usize::MAX))
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|row| {
                        let docket: Option<Vec<String>> = row.get(5);
                        MissingSummaryRow {
                            decision_id: row.get(0),
                            public_id: row.get(1),
                            jurisdiction_type: row.get(2),
                            jurisdiction_name: row.get(3),
                            date_lecture: row.get(4),
                            docket_numbers: docket.filter(|v| !v.is_empty()),
                        }
                    })
                    .collect()
            })
            .collect())
    }

    /// What one engine wrote, oldest id first, in one jurisdiction type or in all
    /// of them. Keyset on `d.id`, batches materialised like the sweep above.
    ///
    /// No version predicate: a text that changes without its generation moving
    /// leaves `summary_prompt_version` equal on both sides, and the engine code
    /// alone separates the two. The run rewrites that code, so a row already
    /// repassed leaves the selection and an interrupted run resumes from zero.
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "db.iter_summaries_by_engine", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_summaries_by_engine(
        &self,
        engine: i16,
        jurisdiction_type: Option<&str>,
        batch_size: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Vec<MissingSummaryRow>>> {
        let mut emitted: i64 = 0;
        let mut last_id: i64 = 0;
        let mut batches: Vec<Vec<MissingSummaryRow>> = Vec::new();
        loop {
            let remaining = match limit {
                None => batch_size,
                Some(lim) => batch_size.min(lim - emitted),
            };
            if remaining <= 0 {
                break;
            }
            let rows = self
                .conn
                .query(
                    &format!(
                        "
                SELECT d.id, d.public_id, d.jurisdiction_type, j.label,
                       d.date_lecture::text, d.docket_numbers
                FROM decisions d
                JOIN decision_text t ON t.decision_id = d.id AND {read}
                LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                WHERE d.id > $1
                  AND t.summary_engine = $2
                  AND ($3::text IS NULL OR d.jurisdiction_type = $3)
                  AND d.public_id IS NOT NULL
                  AND d.deleted_at IS NULL
                ORDER BY d.id
                LIMIT $4
                ",
                        read = super::pick::read("t")
                    ),
                    &[&last_id, &engine, &jurisdiction_type, &remaining],
                )
                .await?;
            let n = rows.len();
            if n == 0 {
                break;
            }
            let mut batch: Vec<MissingSummaryRow> = Vec::with_capacity(n);
            for row in &rows {
                let docket: Option<Vec<String>> = row.get(5);
                batch.push(MissingSummaryRow {
                    decision_id: row.get(0),
                    public_id: row.get(1),
                    jurisdiction_type: row.get(2),
                    jurisdiction_name: row.get(3),
                    date_lecture: row.get(4),
                    docket_numbers: docket.filter(|v| !v.is_empty()),
                });
            }
            emitted += n as i64;
            last_id = rows[n - 1].get::<_, i64>(0);
            batches.push(batch);
            if (n as i64) < remaining {
                break;
            }
        }
        Ok(batches)
    }

    /// Active provenances `(decision_id, source)` on a repeatable
    /// `TABLESAMPLE SYSTEM` of `sample_pct` percent of the pages.
    pub async fn sample_provenances(&self, sample_pct: f32) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT decision_id, source FROM decision_sources TABLESAMPLE SYSTEM ($1) \
                 REPEATABLE (0) WHERE deleted_at IS NULL",
                &[&sample_pct],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Up to `limit` active provenances `(decision_id, source)` drawn on a hash
    /// of the `source_uid`s in `[from, to)`, a range the unique index serves.
    pub async fn sample_provenances_in_uid_range(
        &self,
        from: &str,
        to: &str,
        limit: i64,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT decision_id, source FROM decision_sources \
                 WHERE source_uid >= $1 AND source_uid < $2 AND deleted_at IS NULL \
                 ORDER BY hashtext(source_uid) LIMIT $3",
                &[&from, &to, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// [`DisplayInput`] of each live decision in `ids` or `public_ids`, its
    /// provenance ranked as the decision page ranks it.
    pub async fn fetch_display_inputs(
        &self,
        ids: &[i64],
        public_ids: &[String],
    ) -> Result<Vec<DisplayInput>> {
        let rows = self
            .conn
            .query(
                &format!("
                SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code,
                  {text},
                  ds.source, ds.source_uid, ds.source_fields
                FROM decisions d
                LEFT JOIN LATERAL (
                    SELECT source_fields, source, source_uid
                    FROM decision_sources
                    WHERE decision_id = d.id AND deleted_at IS NULL
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, (lang = 'eng') IS TRUE DESC, source_uid ASC
                    LIMIT 1
                ) ds ON true
                WHERE (d.id = ANY($1) OR d.public_id = ANY($2)) AND d.deleted_at IS NULL
                ", text = super::pick::text("p.full_text", "d")),
                &[&ids, &public_ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| DisplayInput {
                id: r.get(0),
                public_id: r.get(1),
                jurisdiction_type: r.get(2),
                jurisdiction_code: r.get(3),
                full_text: r.get(4),
                source: r.get(5),
                source_uid: r.get(6),
                source_fields: r.get(7),
            })
            .collect())
    }
}

fn codepoints(start: Option<i32>, end: Option<i32>) -> Option<Range<usize>> {
    Some(start? as usize..end? as usize)
}

fn codepoint_columns(span: Option<Range<usize>>) -> (Option<i32>, Option<i32>) {
    span.map_or((None, None), |q| (Some(q.start as i32), Some(q.end as i32)))
}
