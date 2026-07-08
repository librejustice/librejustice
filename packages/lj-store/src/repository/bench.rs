//! Lectures/écritures hors chemin d'upsert : format de source (ADR 0085),
//! résolution `public_id`↔`id`, corps/texte pour les bancs offline, backfills de
//! reprise (re-extract, summary), génération et service des sitemaps.

use super::support::now;
use super::types::{GtDoc, MissingSummaryRow, SitemapRow};
use super::DecisionRepository;
use crate::error::Result;
use chrono::{DateTime, NaiveDate, Utc};
use lj_core::{CERTIFIER_VERSION, EXTRACT_VERSION};
use serde_json::Value;

/// Champs d'extraction d'une révision chargée manuellement (annotation versée
/// à une `extract_version` explicite, ADR 0148 §3) — mêmes colonnes de
/// `decisions` que l'extraction déterministe, seule la version diffère.
/// Périmètre = le **schéma cible** uniquement (dates, dockets, publication +
/// uids de référentiels, ADR 0148) : la version couvre TOUT ce qui est extrait
/// — champs, facettes, citations — d'un bloc. Le parsing du format
/// d'annotation vit chez l'appelant (`lj-bench`) ; ici n'arrive que la
/// projection typée conforme au schéma.
#[derive(Debug, Clone, Default)]
pub struct ManualFields {
    pub date_lecture: Option<NaiveDate>,
    pub date_audience: Option<NaiveDate>,
    pub docket_numbers: Vec<String>,
    pub publication_codes: Vec<String>,
    pub solution_uid: Option<String>,
    pub voie_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    pub jurisdiction_code: Option<String>,
}

impl DecisionRepository<'_> {
    /// Persiste le `payload_format` (`xml`/`json`) d'une décision. Le payload
    /// source brut a été droppé (ADR 0085, reconstructible depuis `full_text` +
    /// `source_fields`) ; seul le format reste nécessaire pour choisir le
    /// reconstructeur côté re-extract (`fetch_reextract_inputs`).
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

    /// Métadonnées d'identification par `decision_id` : `(id, public_id,
    /// search_title)`. Sens inverse de [`Self::resolve_public_ids`] — utilisé par
    /// le banc (`lj-bench rank-arms`) pour étiqueter les docs poolés à juger.
    /// Les décisions sans `public_id` sont omises (jamais poolables côté API).
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

    /// Texte intégral par `decision_id` : `(id, public_id, search_title,
    /// full_text)`. Utilisé par le banc (`lj-bench dump-bodies`) pour
    /// matérialiser les corps des docs à juger d'une campagne de complétion GT.
    /// Lit `decisions.full_text` (grain décision, ADR 0084) — plus la
    /// recombinaison des chunks. Décisions sans `public_id`/`full_text` omises.
    pub async fn fetch_bodies(&self, ids: &[i64]) -> Result<Vec<(i64, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.public_id, COALESCE(d.search_title, ''), d.full_text \
                 FROM decisions d \
                 WHERE d.id = ANY($1) AND d.public_id IS NOT NULL AND d.full_text IS NOT NULL",
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

    /// Texte intégral (`decisions.full_text`, grain décision ADR 0085) par id —
    /// banc BM25 offline (`lj-bench rank-bsweep`). Lit le texte indexé tel quel,
    /// pas la recombinaison des chunks de [`Self::fetch_bodies`]. Décisions sans
    /// `full_text` omises.
    pub async fn fetch_full_texts(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, full_text FROM decisions \
                 WHERE id = ANY($1) AND full_text IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Corps + métadonnées grain décision pour la matérialisation GT
    /// (`lj-bench gt-pool`) : `full_text` indexé (ADR 0085, pas la recombinaison
    /// chunks de [`Self::fetch_bodies`]) + juridiction/niveau/date/titre pour le
    /// `_meta_<slug>.yaml`. Décisions sans `full_text` omises.
    pub async fn fetch_gt_docs(&self, ids: &[i64]) -> Result<Vec<GtDoc>> {
        let rows = self
            .conn
            .query(
                "SELECT id, public_id, COALESCE(jurisdiction_name, ''), \
                 COALESCE(juridiction_type, ''), COALESCE(date_lecture::text, ''), \
                 COALESCE(search_title, ''), full_text FROM decisions \
                 WHERE id = ANY($1) AND public_id IS NOT NULL AND full_text IS NOT NULL",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| GtDoc {
                id: r.get(0),
                public_id: r.get(1),
                jurisdiction_name: r.get(2),
                juridiction_type: r.get(3),
                date_lecture: r.get(4),
                search_title: r.get(5),
                full_text: r.get(6),
            })
            .collect())
    }

    /// Échantillon aléatoire de `full_text` (`TABLESAMPLE SYSTEM`) — banc BM25
    /// offline (`lj-bench rank-bsweep`) : sert à estimer la longueur moyenne
    /// (avgdl) du champ indexé, tokenisée côté banc avec le tokenizer de l'index.
    /// `sample_pct` est piloté par le code (jamais une entrée externe).
    pub async fn sample_full_texts(&self, sample_pct: f64) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT full_text FROM decisions TABLESAMPLE SYSTEM ({sample_pct}) \
             WHERE full_text IS NOT NULL"
        );
        let rows = self.conn.query(sql.as_str(), &[]).await?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    /// Fréquence documentaire `df` d'un terme indexé `term` dans `full_text` —
    /// banc BM25 offline (`lj-bench rank-bsweep`), pour l'IDF. `term` est déjà
    /// folded/lowercased comme le tokenizer de l'index ; `paradedb.term` ne le
    /// re-tokenise pas (comparaison directe aux termes de `decisions_bm25`).
    pub async fn bm25_term_doc_count(&self, term: &str) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*) FROM decisions \
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

    /// Entrées du re-extract (ADR 0085) : `(payload_format, full_text,
    /// source_fields, source_uid)` d'une décision, pour reconstruire le payload
    /// source via `reconstruct_{json,xml}_payload` et re-parser. `full_text` est
    /// le texte canonique (`decisions`) ; `payload_format`/`source_fields`/
    /// `source_uid` viennent de la **provenance autoritaire** (ADR 0098 §4.2 :
    /// rang max active) — celle dont le texte EST `full_text`. `None` si pas de
    /// provenance active ou si `full_text`/`source_fields` manque.
    pub async fn fetch_reextract_inputs(
        &self,
        decision_id: i64,
    ) -> Result<Option<(String, Value, String)>> {
        let row = self
            .conn
            .query_opt(
                "
                SELECT d.full_text, ds.source_fields, ds.source_uid
                FROM decisions d
                JOIN LATERAL (
                    SELECT source_fields, source_uid
                    FROM decision_sources
                    WHERE decision_id = d.id AND deleted_at IS NULL
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC
                    LIMIT 1
                ) ds ON true
                WHERE d.id = $1
                ",
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

    /// Entrées de re-extraction d'un **lot de `decision_id`** en **une seule
    /// requête** (`d.id = ANY($1)`), au lieu d'un aller-retour par id. Même
    /// projection que [`Self::fetch_reextract_inputs`] (provenance autoritaire via
    /// `JOIN LATERAL`), plus l'`id` en tête pour ré-apparier côté appelant. Les ids
    /// sans provenance active / `full_text`/`source_fields` manquant sont omis ;
    /// l'ordre n'est pas garanti (l'appelant ré-apparie par `id`). Read-only.
    pub async fn fetch_reextract_inputs_batch(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, String, Value, String)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id, d.full_text, ds.source_fields, ds.source_uid
                FROM decisions d
                JOIN LATERAL (
                    SELECT source_fields, source_uid
                    FROM decision_sources
                    WHERE decision_id = d.id AND deleted_at IS NULL
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC
                    LIMIT 1
                ) ds ON true
                WHERE d.id = ANY($1)
                  AND d.full_text IS NOT NULL
                  AND ds.source_fields IS NOT NULL
                ",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let full_text: Option<String> = r.get(1);
                let source_fields: Option<Value> = r.get(2);
                match (full_text, source_fields) {
                    (Some(ft), Some(sf)) => {
                        Some((r.get::<_, i64>(0), ft, sf, r.get::<_, String>(3)))
                    }
                    _ => None,
                }
            })
            .collect())
    }

    /// Entrées de re-extraction adressées **par `source_uid`** (et non par
    /// `decision_id`) : pour le banc d'extraction, dont la GT est clé sur le
    /// `source_uid` exact de la provenance annotée. Renvoie, pour chaque uid
    /// trouvé avec un `full_text` + `source_fields` non nuls, le triplet
    /// `(source_uid, full_text, source_fields)` — la matière de
    /// [`lj_core::parsing::Decision::from_source_fields`] (ADR 0085). Batch
    /// `= ANY($1)` ; les uids absents / incomplets sont simplement omis.
    pub async fn fetch_reextract_inputs_by_source_uids(
        &self,
        source_uids: &[String],
    ) -> Result<Vec<(String, String, Value)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT ds.source_uid, d.full_text, ds.source_fields
                FROM decision_sources ds
                JOIN decisions d ON d.id = ds.decision_id
                WHERE ds.source_uid = ANY($1)
                  AND ds.deleted_at IS NULL
                  AND d.deleted_at IS NULL
                  AND d.full_text IS NOT NULL
                  AND ds.source_fields IS NOT NULL
                ",
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

    /// Keyset (par `decision_id`) des décisions **multi-provenances tout-judilibre**
    /// (≥2 `decision_sources` actifs, tous `source='judilibre'`) avec, pour chacune,
    /// la liste de ses provenances `(source_uid, payload_format, source_fields)` —
    /// la matière de la détection des faux merges (#29 / ADR 0100) : recalculer
    /// `canonical_ref` par provenance et flaguer les décisions dont les clés
    /// divergent. Read-only. Renvoie les lignes triées `(decision_id, ds.id)` :
    /// l'appelant regroupe par `decision_id`. `last_id` = dernier `decision_id`
    /// traité (reprise).
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

    /// Keyset (par `decision_id`) des décisions ayant **au moins un chunk sans
    /// embedding** (orphelins #39), avec, pour chacune, les entrées de
    /// reconstruction (ADR 0085) **plus** le `public_id` et le `content_checksum`
    /// existants — pour rebâtir un `Candidate` fidèle qui re-traverse le pipeline
    /// (re-chunk + embed) **sans changer l'identité ni le checksum** (le re-embed
    /// déclenché par la garde `require_embeddings && !has_embeddings`). Driven par
    /// `decision_chunks.embedding IS NULL` (volume minuscule, ~1.8k).
    ///
    /// Le keyset est porté par le CTE `ids` (LEFT JOIN sur décision + provenance) :
    /// chaque `decision_id` candidat est **toujours** renvoyé (l'appelant avance
    /// `last_id` au max du lot), avec ses champs de reconstruction en `Option` —
    /// `None` si la décision n'a pas de provenance active / pas de `full_text`
    /// (lignes incomplètes ignorées à la reconstruction, sans bloquer la reprise).
    /// `last_id` = dernier `decision_id` traité (reprise). Renvoie
    /// `(id, Option<(public_id, payload_format, full_text, source_fields,
    /// source_uid, content_checksum)>)`.
    #[allow(clippy::type_complexity)]
    pub async fn fetch_missing_embedding_batch(
        &self,
        last_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, Option<(String, String, String, Value, String, String)>)>> {
        let rows = self
            .conn
            .query(
                "
                WITH ids AS (
                    SELECT DISTINCT c.decision_id AS id
                    FROM decision_chunks c
                    WHERE c.embedding IS NULL AND c.decision_id > $1
                    ORDER BY c.decision_id
                    LIMIT $2
                )
                SELECT ids.id, d.public_id, d.full_text,
                       ds.payload_format, ds.source_fields, ds.source_uid, ds.content_checksum
                FROM ids
                LEFT JOIN decisions d ON d.id = ids.id
                LEFT JOIN LATERAL (
                    SELECT payload_format, source_fields, source_uid, content_checksum
                    FROM decision_sources
                    WHERE decision_id = ids.id AND deleted_at IS NULL
                    ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC
                    LIMIT 1
                ) ds ON true
                ORDER BY ids.id
                ",
                &[&last_id, &limit],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.get(0);
            let public_id: Option<String> = r.get(1);
            let full_text: Option<String> = r.get(2);
            let payload_format: Option<String> = r.get(3);
            let source_fields: Option<Value> = r.get(4);
            let source_uid: Option<String> = r.get(5);
            let content_checksum: Option<String> = r.get(6);
            let complete = match (
                public_id,
                full_text,
                payload_format,
                source_fields,
                source_uid,
                content_checksum,
            ) {
                (Some(pid), Some(ft), Some(fmt), Some(sf), Some(uid), Some(ck)) => {
                    Some((pid, fmt, ft, sf, uid, ck))
                }
                _ => None,
            };
            out.push((id, complete));
        }
        Ok(out)
    }

    /// Keyset des décisions à re-parser : `full_text` présent et
    /// ``extract_version`` strictement sous la version courante du pipeline
    /// (reprise de ``reextract-fields``, ADR 0083) — les versions ≥ courante,
    /// dont les révisions manuelles (> courante par convention), sont hors
    /// worklist. Le re-extract part de la reconstruction `(full_text,
    /// source_fields)` — plus du payload (ADR 0085).
    pub async fn decision_ids_for_reextract(&self, last_id: i64, limit: i64) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.full_text IS NOT NULL
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

    /// Certifie un lot de décisions à `certified_version` (ADR 0125 Inc.2-bis) : le
    /// re-extract par défaut les skippera tant que `certified_version >= CERTIFIER_VERSION`.
    /// Appelé par l'oracle haut-recall (Inc.2) sur les décisions dont la capture est
    /// prouvée complète + résolue. Renvoie le nb de lignes mises à jour. **Setter
    /// uniquement** : invalidation = bump de `CERTIFIER_VERSION` (comparaison, sans write).
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

    /// Nombre de décisions **périmées** (`extract_version` ≠ courante, `full_text`
    /// présent) : la **traîne** restant à ré-extraire par le mode par défaut de
    /// `reextract-fields` (même prédicat que [`Self::decision_ids_for_reextract`]).
    /// Read-only ; sert à afficher l'avancement `processed/total` + ETA.
    pub async fn count_decisions_for_reextract(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "
                SELECT count(*) FROM decisions
                WHERE full_text IS NOT NULL
                  AND (extract_version IS NULL OR extract_version < $1)
                  AND (certified_version IS NULL OR certified_version < $2)
                ",
                &[&EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(row.get(0))
    }

    /// **Worklist complète** des ids de décisions périmées (même prédicat que
    /// [`Self::count_decisions_for_reextract`]), triée par id. Un seul scan (~5 s
    /// sur 3,6 M) qui remplace les milliers de keyset filtrés `id > cursor AND
    /// extract_version != courant` — ces derniers dégradent (l'index PK élimine un
    /// préfixe v8 croissant). La liste (~8 Mo/M ids) est ensuite partitionnée entre
    /// workers concurrents. À 25 % de lignes périmées le seq scan est optimal (un
    /// index ne serait pas retenu) ; il ne paierait qu'à forte convergence, où il
    /// reste peu de travail.
    pub async fn stale_decision_ids_for_reextract(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT id FROM decisions
                WHERE full_text IS NOT NULL
                  AND (extract_version IS NULL OR extract_version < $1)
                  AND (certified_version IS NULL OR certified_version < $2)
                ORDER BY id
                ",
                &[&EXTRACT_VERSION, &CERTIFIER_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Worklist de la **passe intégrale** (ADR 0145, `--full`) : TOUT le fonds
    /// jusqu'à la version courante incluse — même déjà à jour, même certifié —
    /// c'est le relink hebdomadaire (le `LinkSnapshot` du run voit le
    /// catalogue du jour ; le skip Rust des sets de citations inchangés borne
    /// les écritures au delta). Les révisions manuelles (version > courante)
    /// restent hors worklist.
    pub async fn all_decision_ids_for_reextract(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT id FROM decisions
                WHERE full_text IS NOT NULL
                  AND (extract_version IS NULL OR extract_version <= $1)
                ORDER BY id
                ",
                &[&EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Écrit une révision des champs d'extraction **dans les colonnes de
    /// `decisions`** — schéma cible + uids de facettes — stampée
    /// `extract_version = version` : même modèle que l'extraction
    /// déterministe, seule la version diffère (une version > courante n'est
    /// jamais réécrite par le recognizer). Les champs absents/`None` écrivent
    /// NULL (révision exhaustive : absent = confirmé absent), sauf
    /// `jurisdiction_code` (COALESCE : porté aussi par les métadonnées
    /// source, jamais dégradé par une annotation muette). Renvoie `false` si
    /// aucune décision ne matche le `source_uid` ou si la décision porte une
    /// version supérieure (invariant : jamais dégrader une `extract_version`
    /// plus grande).
    pub async fn upsert_fields_at_version(
        &self,
        source_uid: &str,
        version: i16,
        fields: &ManualFields,
    ) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE decisions d SET
                     date_lecture = $2, date_audience = $3, docket_numbers = $4,
                     publication_codes = $5,
                     solution_uid = $6, voie_uid = $7, office_uid = $8,
                     legal_domain_uid = $9,
                     jurisdiction_code = COALESCE($10, d.jurisdiction_code),
                     extract_version = $11, updated_at = now()
                 FROM decision_sources ds
                 WHERE ds.decision_id = d.id AND ds.source_uid = $1
                   AND (d.extract_version IS NULL OR d.extract_version <= $11)",
                &[
                    &source_uid,
                    &fields.date_lecture,
                    &fields.date_audience,
                    &fields.docket_numbers,
                    &fields.publication_codes,
                    &fields.solution_uid,
                    &fields.voie_uid,
                    &fields.office_uid,
                    &fields.legal_domain_uid,
                    &fields.jurisdiction_code,
                    &version,
                ],
            )
            .await?;
        Ok(n > 0)
    }

    /// Remplace la couche citations d'une décision à une `extract_version`
    /// explicite (ADR 0145 §3) : la révision est exhaustive, le set fourni
    /// EST la vérité — DELETE + COPY via l'écrivain commun. Les `rows`
    /// arrivent déjà validées au bord (parse de l'appelant : bornes, tri,
    /// ancrage `surface == full_text[span]`, cohérence status ↔ cible) —
    /// même type que le recognizer, seule la version diffère. Renvoie `None`
    /// si aucun `source_uid` ne matche ou si la décision porte une version
    /// supérieure (jamais dégrader), sinon le nombre de lignes insérées.
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

    /// Keyset des décisions à re-parser **ciblées par `juridiction_type`**,
    /// **indépendamment de `extract_version`** (ré-extraction d'un comportement
    /// nouveau sur un sous-ensemble — ex. famille générique CNDA/CEDH/CJUE/CONSTIT/TC
    /// après câblage des citations, ADR 0102 §B) **sans bump global** de
    /// `EXTRACT_VERSION` (qui re-parserait 3,9 M décisions). `full_text` présent.
    /// Les révisions manuelles (`extract_version` > courante) restent exclues.
    pub async fn decision_ids_for_reextract_by_juridiction(
        &self,
        last_id: i64,
        limit: i64,
        juridiction_types: &[String],
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.full_text IS NOT NULL
                  AND d.juridiction_type = ANY($3)
                  AND (d.extract_version IS NULL OR d.extract_version <= $4)
                ORDER BY d.id
                LIMIT $2
                ",
                &[&last_id, &limit, &juridiction_types, &EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Keyset des décisions à re-parser **ciblées par un texte cité**
    /// (`ref_text_uid`, ADR 0145 M4), **indépendamment de `extract_version`**.
    /// Pour rejouer extract+link sur le seul gisement d'un instrument, sans bump
    /// global ni re-parse de tout un `juridiction_type`. `full_text` présent,
    /// révisions manuelles (`extract_version` > courante) exclues. À combiner
    /// avec `--field legal_references` (ne touche que les citations, via
    /// `replace_citations`).
    pub async fn decision_ids_for_reextract_by_citing_ref_uid(
        &self,
        last_id: i64,
        limit: i64,
        ref_text_uid: &str,
    ) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "
                SELECT d.id
                FROM decisions d
                WHERE d.id > $1
                  AND d.full_text IS NOT NULL
                  AND (d.extract_version IS NULL OR d.extract_version <= $4)
                  AND EXISTS (
                      SELECT 1 FROM legal_citation lc
                      WHERE lc.decision_id = d.id AND lc.ref_text_uid = $3
                  )
                ORDER BY d.id
                LIMIT $2
                ",
                &[&last_id, &limit, &ref_text_uid, &EXTRACT_VERSION],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    #[tracing::instrument(name = "db.set_summary", skip(self, summary), fields(db.system = "postgresql"))]
    pub async fn set_summary(
        &self,
        decision_id: i64,
        summary: &str,
        prompt_version: i16,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE decisions SET summary = $1, summary_prompt_version = $2, \
                 updated_at = $3 WHERE id = $4",
                &[&summary, &prompt_version, &now(), &decision_id],
            )
            .await?;
        Ok(())
    }

    /// Itère `(public_id, lastmod)` pour toutes les décisions, en ordre PK.
    ///
    /// Port de `iter_decisions_for_sitemap`. `lastmod` est
    /// `GREATEST(date_lecture, updated_at::date)` côté SQL : `date_lecture`
    /// quand connue (cas dominant), sinon le timestamp d'ingestion. On exclut
    /// les décisions sans `public_id`. Pagination keyset par `d.id` — le
    /// générateur Python est matérialisé ici en `Vec` (consommé en une passe
    /// par `build_sitemaps`).
    #[tracing::instrument(name = "db.iter_decisions_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_decisions_for_sitemap(
        &self,
        batch_size: i64,
    ) -> Result<Vec<(String, NaiveDate)>> {
        let mut out: Vec<(String, NaiveDate)> = Vec::new();
        let mut last_id: i64 = 0;
        loop {
            let rows = self
                .conn
                .query(
                    "
                SELECT id, public_id,
                       GREATEST(date_lecture, updated_at::date) AS lastmod
                FROM decisions
                WHERE public_id IS NOT NULL
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
                let lastmod: NaiveDate = row.get(2);
                out.push((public_id, lastmod));
            }
            last_id = rows[rows.len() - 1].get::<_, i64>(0);
            if (rows.len() as i64) < batch_size {
                return Ok(out);
            }
        }
    }

    /// Remplace l'intégralité de la table `sitemaps` par `files` (régénération
    /// complète du cron). `DELETE` global puis `INSERT` de chaque ligne : les
    /// sub-sitemaps d'un run précédent (corpus rétréci) disparaissent
    /// automatiquement — équivalent du `sweep_orphans` R2 d'hier. L'appelant
    /// wrappe l'appel dans une transaction (cf. contrat repo, comme le pipeline).
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

    /// Lit un sitemap par nom de fichier — `(body, content_type)`, `None` si
    /// absent. Sert les routes `/sitemap.xml` + `/sitemap-{n}.xml.gz` de
    /// `lj-server`.
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

    /// Itère les `public_id` des décisions dont `updated_at >= since`.
    ///
    /// Port de `iter_public_ids_updated_since`. Sert le push IndexNow
    /// post-ingest (ADR 0044) : couvre les décisions nouvellement upsertées ET
    /// celles dont le `summary` vient d'être (re)généré (`set_summary` bumpe
    /// `updated_at`). Pagination keyset par `id`, matérialisée en `Vec`.
    #[tracing::instrument(name = "db.iter_public_ids_updated_since", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_public_ids_updated_since(
        &self,
        since: DateTime<Utc>,
        batch_size: i64,
    ) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut last_id: i64 = 0;
        loop {
            let rows = self
                .conn
                .query(
                    "
                SELECT id, public_id
                FROM decisions
                WHERE public_id IS NOT NULL
                  AND updated_at >= $1
                  AND id > $2
                ORDER BY id
                LIMIT $3
                ",
                    &[&since, &last_id, &batch_size],
                )
                .await?;
            if rows.is_empty() {
                return Ok(out);
            }
            for row in &rows {
                out.push(row.get(1));
            }
            last_id = rows[rows.len() - 1].get::<_, i64>(0);
            if (rows.len() as i64) < batch_size {
                return Ok(out);
            }
        }
    }

    /// Itère les décisions sans summary à jour, par batches.
    ///
    /// Port de `iter_decisions_missing_summary`. Chaque ligne =
    /// `(decision_id, public_id, juridiction_type, jurisdiction_name,
    /// date_lecture, docket_numbers)` — les quatre dernières servent à
    /// reconstruire le titre côté appelant. Sélectionne les rows dont `summary
    /// IS NULL` OU `summary_prompt_version < target_version`, en excluant les
    /// tombstones (`deleted_at IS NOT NULL` — vidés RGPD, jamais résumables) et
    /// les décisions sans corps (`full_text IS NULL`) : sans ce filtre la
    /// requête re-scanne à chaque run ~34 k tombstones que l'aval écarte en
    /// `no_body`. `deleted_at IS NULL` est servi par l'index partiel
    /// `idx_decisions_active`. Pagination keyset
    /// stable sur `ORDER BY d.id`. `limit`, s'il est fourni, borne le nombre
    /// total de lignes yieldées sur toute l'itération. `wrap` : avec
    /// `start_id > 0`, une fois l'arc `]start_id, max]` épuisé, reprend sur
    /// `]0, start_id]` (sampling de review).
    ///
    /// Renvoie tous les batches matérialisés (l'appelant les traite en
    /// séquence ; le générateur Python streame, mais les frontières de batch
    /// — donc l'identité des appels Mistral concurrents — sont préservées).
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "db.iter_decisions_missing_summary", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_decisions_missing_summary(
        &self,
        target_version: i16,
        batch_size: i64,
        limit: Option<i64>,
        start_id: i64,
        wrap: bool,
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
                    "
                SELECT d.id, d.public_id, d.juridiction_type, d.jurisdiction_name,
                       d.date_lecture::text, d.docket_numbers
                FROM decisions d
                WHERE d.id > $1
                  AND d.id <= $2
                  AND (
                    d.summary IS NULL
                    OR d.summary_prompt_version IS NULL
                    OR d.summary_prompt_version < $3
                  )
                  AND d.public_id IS NOT NULL
                  AND d.deleted_at IS NULL
                  AND d.full_text IS NOT NULL
                ORDER BY d.id
                LIMIT $4
                ",
                    &[&last_id, &ceiling, &target_version, &remaining],
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
                        juridiction_type: row.get(2),
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
                // Arc courant épuisé. En mode wrap, on rejoue une fois sur le
                // bas de la plage `]0, start_id]` ; sinon on s'arrête.
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
}
