//! Provenance, résolution d'identité et déduplication (ADR 0080 / 0098 / 0100) :
//! sondes d'idempotence, backfills `decision_sources`/`ecli`/`canonical_ref`,
//! fusion de clusters doublons, retrait par provenance (tombstone) + reconcile.

use std::collections::{HashMap, HashSet};

use super::support::{now, source_from_source_uid};
use super::types::ExistingDecisionState;
use super::DecisionRepository;
use crate::error::Result;
use serde_json::Value;

impl DecisionRepository<'_> {
    /// Sonde d'idempotence intra-source (ADR 0098 §4) sur le pivot
    /// `decision_sources.source_uid` (UNIQUE). Renvoie `(decision_id,
    /// content_checksum, active)` de la provenance — **tombstonée comprise**
    /// (`active = deleted_at IS NULL`) : un `source_uid` connu mais tombstoné se
    /// ré-update en place, même décision (§4.1, résurrection). `None` =
    /// provenance jamais vue → résolution d'identité (ECLI/canonical_ref) en aval.
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

    /// Cible de fusion par `canonical_ref` (ADR 0100/0104) respectant l'invariant
    /// *≤1 provenance active par source et par décision* : `min(id)` parmi les
    /// décisions actives portant cette clé qui **ne portent pas déjà** une
    /// provenance active de `incoming_source`. `None` → aucune cible compatible
    /// (clé inconnue, ou toutes les décisions de la clé portent déjà cette source)
    /// → l'appelant crée une **nouvelle** décision. On ne fusionne donc **jamais**
    /// deux provenances same-source par `canonical_ref` (clé non unique → perte de
    /// texte irréversible si faux merge, ADR 0104) ; le merge cross-source reste
    /// autorisé. Index partiel `idx_decisions_canonical_ref`.
    #[tracing::instrument(name = "db.find_canonical_ref_merge_target", skip(self), fields(db.system = "postgresql"))]
    pub async fn find_canonical_ref_merge_target(
        &self,
        canonical_ref: &str,
        incoming_source: &str,
    ) -> Result<Option<i64>> {
        let row = self
            .conn
            .query_one(
                "SELECT min(d.id) FROM decisions d \
                 WHERE d.canonical_ref = $1 AND d.deleted_at IS NULL \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM decision_sources s \
                     WHERE s.decision_id = d.id AND s.source = $2 \
                       AND s.deleted_at IS NULL)",
                &[&canonical_ref, &incoming_source],
            )
            .await?;
        Ok(row.get::<_, Option<i64>>(0))
    }

    /// Résout une décision active par ECLI via l'index partiel `idx_decisions_ecli`
    /// (ADR 0080/0107). L'ECLI **n'est pas unique** (judilibre réutilise un même
    /// ECLI sur de vieux arrêts distincts) : on ne fusionne que sur un ECLI **non
    /// ambigu** — exactement **une** décision active — sinon `None` (repli sur
    /// `canonical_ref`, jamais de fusion à l'aveugle). `None` aussi si l'ECLI est
    /// inconnu.
    #[tracing::instrument(name = "db.find_decision_by_ecli", skip(self), fields(db.system = "postgresql"))]
    pub async fn find_decision_by_ecli(&self, ecli: &str) -> Result<Option<i64>> {
        let rows = self
            .conn
            .query(
                "SELECT id FROM decisions WHERE ecli = $1 AND deleted_at IS NULL LIMIT 2",
                &[&ecli],
            )
            .await?;
        match rows.as_slice() {
            [one] => Ok(Some(one.get::<_, i64>(0))),
            _ => Ok(None),
        }
    }

    /// `source_uid` de la provenance **autoritaire** d'une décision (ADR 0098
    /// §4.2, ADR 0127/0153) : rang max, puis rendition **FR-prioritaire**
    /// (`lang = 'fra'` DESC), puis id croissant, **tombstonée comprise** (même
    /// définition que [`reconcile`](Self::reconcile)). `None` si la décision n'a
    /// aucune provenance. Sert à décider de l'overwrite du contenu canonique :
    /// seule l'autorité écrit `full_text`/métadonnées (§3).
    #[tracing::instrument(name = "db.authority_source_uid", skip(self), fields(db.system = "postgresql"))]
    pub(super) async fn authority_source_uid(&self, decision_id: i64) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT source_uid FROM decision_sources WHERE decision_id = $1 \
                 ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC LIMIT 1",
                &[&decision_id],
            )
            .await?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    }

    /// Upsert idempotent (#7) d'une ligne `decision_sources` (provenance) sur sa
    /// clé naturelle `source_uid UNIQUE` (ADR 0098 §2). Le `source` est dérivé du
    /// `source_uid` ; `source_rank` est une colonne **générée** depuis `source`
    /// (ADR 0113), jamais écrite ici. Rattache la provenance à `decision_id`, y
    /// **porte le `source_fields`** (payload méta par provenance, propriétaire
    /// unique de cette donnée après le DROP des colonnes mono-source de
    /// `decisions`) et **lève le tombstone** (`deleted_at = NULL`) : un
    /// `source_uid` connu mais tombstoné se ré-active à la ré-ingestion (§4.1,
    /// résurrection, même décision). Rejoué sans dupliquer.
    #[tracing::instrument(name = "db.upsert_decision_source", skip(self, source_fields), fields(db.system = "postgresql"))]
    pub async fn upsert_decision_source(
        &self,
        decision_id: i64,
        source_uid: &str,
        content_checksum: &str,
        payload_format: &str,
        source_fields: &Value,
    ) -> Result<()> {
        // `source_rank` est une colonne **générée** depuis `source` (ADR 0113) —
        // jamais écrite ici (sinon erreur « cannot insert into generated column »).
        let source = source_from_source_uid(source_uid);
        // `lang` (ADR 0153) = langue de la source, **dite par l'ingester** dans
        // `source_fields["lang"]` (CEDH/CJUE ; 'fra'/'eng'), matérialisée en colonne.
        // Absente pour les sources FR-only → NULL.
        let lang = source_fields.get("lang").and_then(|v| v.as_str());
        self.conn
            .execute(
                "
                INSERT INTO decision_sources
                  (decision_id, source, source_uid, content_checksum,
                   payload_format, source_fields, lang)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (source_uid) DO UPDATE SET
                  decision_id = EXCLUDED.decision_id,
                  source = EXCLUDED.source,
                  content_checksum = EXCLUDED.content_checksum,
                  payload_format = EXCLUDED.payload_format,
                  source_fields = EXCLUDED.source_fields,
                  lang = EXCLUDED.lang,
                  deleted_at = NULL,
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

    /// Backfill batché (keyset par `decisions.id`) de la colonne `decisions.ecli`
    /// depuis `decision_sources.source_fields->>'ecli'` de la provenance
    /// autoritaire (ADR 0093/0098) : Judilibre porte l'ECLI dans son payload mais
    /// la colonne `decisions.ecli` est NULL sur les lignes existantes, ce qui rend
    /// la dédup ECLI-first (ADR 0080) inerte. Une ligne
    /// par `decisions` au-delà de `after_id`, dans l'ordre d'id, plafonné à
    /// `batch`. Idempotent : n'écrit que les lignes dont `ecli IS NULL` et dont
    /// `source_fields` porte la clé `ecli` — un ECLI déjà posé n'est jamais écrasé.
    /// L'ECLI **n'étant pas unique** (ADR 0107, index non unique), un même ECLI peut
    /// se poser sur plusieurs décisions distinctes (réutilisation judilibre) sans
    /// échec ; la dédup at-ingest ne fusionne que sur un ECLI non ambigu (cf.
    /// [`find_decision_by_ecli`](Self::find_decision_by_ecli)). Renvoie
    /// `(rows_updated, last_id)` où `last_id` est l'id max du lot lu (curseur de
    /// reprise), `None` quand le lot est vide (épuisement).
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
                        ORDER BY s.source_rank DESC, (s.lang = 'fra') IS TRUE DESC, s.id ASC
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
    /// calculer (ADR 0100) : `canonical_ref IS NULL` et `full_text` présent (la
    /// reconstruction `(full_text, source_fields)` en dépend, ADR 0085). Renvoie
    /// les ids du lot au-delà de `after_id`, dans l'ordre d'id, plafonné à
    /// `limit`. Une décision sans clé exploitable (discriminants manquants) reste
    /// `NULL` et sera relue à chaque rerun — résidu minoritaire, acceptable pour
    /// un backfill one-shot.
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
                  AND d.full_text IS NOT NULL
                ORDER BY d.id
                LIMIT $2
                ",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Keyset (par `decisions.id`) de **toutes** les décisions à `canonical_ref`
    /// **recalculable** (`full_text` présent) — pour un **re-dérive forcé** (ADR
    /// 0103 / fix cross-court 2026-06-15) : les clés historiques au format 3-champs
    /// `{nom}|{rg}|{date}` doivent passer en 4-champs `{type}|{location}|{rg}|{date}`
    /// (la colonne n'étant plus `NULL`, le backfill normal ne les revisite pas).
    /// Une clé recalculée `None` (discriminant manquant) **laisse l'existante**
    /// (l'écriture en masse n'inclut que les `Some`, cf. `update_canonical_refs_bulk`).
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
                  AND d.full_text IS NOT NULL
                ORDER BY d.id
                LIMIT $2
                ",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Écrit en masse le `canonical_ref` calculé en Rust (ADR 0100), `unnest`
    /// apparié `(id, key)`. N'inclut que les décisions ayant produit une clé —
    /// celles sans clé restent `NULL`. Renvoie le nombre de lignes mises à jour.
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
        // EXISTS corrélés (cf. Python) : évite un re-scan complet de
        // decision_chunks à chaque appel.
        // Idempotence par provenance (ADR 0098 §4) : on lit `source_uid` /
        // `content_checksum` sur `decision_sources` (le pivot), join `decisions`
        // pour `public_id`. **Provenances actives seules** : une provenance
        // tombstonée n'apparaît pas ici → le candidat retombe en survivant et
        // `upsert` la ressuscite par `source_uid` (§4.1).
        let rows = self
            .conn
            .query(
                "
                SELECT
                  ds.source_uid,
                  ds.decision_id,
                  ds.content_checksum,
                  (
                    EXISTS (SELECT 1 FROM decision_chunks ch WHERE ch.decision_id = ds.decision_id)
                    AND NOT EXISTS (
                      SELECT 1 FROM decision_chunks ch
                      WHERE ch.decision_id = ds.decision_id AND ch.embedding IS NULL
                    )
                  ),
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
                    has_embeddings: row.get(3),
                    public_id: row.get(4),
                },
            );
        }
        Ok(out)
    }

    /// Sous-ensemble des `source_uid` **présents** (provenances actives) dont la
    /// rendition servie est le **français** (`lang = 'fra'`, ADR 0153 —
    /// couvre CEDH `languageisocode='FRE'` ET CJUE `resource_obtained_language='fra'`).
    /// Les syncs CEDH/CJUE s'en servent pour ne re-fetcher que les nouveaux et les
    /// EN-only (upgrade FR différé, ADR 0120) : un arrêt déjà en FR est définitif
    /// (texte immuable post-publication), inutile de le re-télécharger.
    #[tracing::instrument(name = "db.find_fr_source_uids", skip(self, source_uids), fields(db.system = "postgresql"))]
    pub async fn find_fr_source_uids(&self, source_uids: &[String]) -> Result<HashSet<String>> {
        if source_uids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows = self
            .conn
            .query(
                "
                SELECT ds.source_uid
                FROM decision_sources ds
                WHERE ds.source_uid = ANY($1) AND ds.deleted_at IS NULL
                  AND ds.lang = 'fra'
                ",
                &[&source_uids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Résout une décision canonique EXISTANTE par identité (ADR 0100) : ECLI
    /// d'abord (autoritaire si présente, interop), sinon `canonical_ref` (couvre
    /// les ~88 % sans ECLI). `None` si aucune identité connue → création.
    pub async fn resolve_identity(
        &self,
        decision: &lj_core::decision::Decision,
        canonical_ref: Option<&str>,
    ) -> Result<Option<i64>> {
        if let Some(ecli) = decision.ecli.as_deref() {
            if let Some(id) = self.find_decision_by_ecli(ecli).await? {
                return Ok(Some(id));
            }
        }
        if let Some(key) = canonical_ref {
            // Invariant ≤1 provenance/source (ADR 0104) : on ne rattache par
            // `canonical_ref` qu'à une décision ne portant **pas déjà** la source
            // entrante — jamais de fusion intra-source (clé non unique → perte de
            // texte irréversible si faux merge). L'ECLI (unique, autoritaire) a
            // déjà été tenté ci-dessus ; le merge cross-source reste permis.
            let source = source_from_source_uid(&decision.source_uid);
            if let Some(id) = self.find_canonical_ref_merge_target(key, source).await? {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Retrait d'une provenance (ADR 0098 §5). **Toujours par `source_uid`**
    /// (provenance exacte, jamais par identité canonique) : tombstone la ligne
    /// `decision_sources` (`deleted_at = NOW()`), puis `reconcile(decision_id)`
    /// recalcule la décision (vidée si l'autorité est tombstonée, active sinon).
    /// Tombstone + reconcile sont **atomiques** (transaction propre) et **sous
    /// verrou** (`pg_advisory_xact_lock` sur `decision_id`) — appelée depuis les
    /// boucles d'application des retraits (DILA, `reverses`, tombstones Judilibre)
    /// qui tournent en autocommit. **On ne supprime plus en régénérant un id** :
    /// le squelette (id, public_id, identité) reste, un ré-update ultérieur se
    /// rejoint par `source_uid`. `source_uid` inconnu → no-op idempotent
    /// (`false`).
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

    /// Corps transactionnel de [`delete`](Self::delete) : résout la provenance,
    /// pose le verrou xact sur `decision_id`, tombstone, `reconcile`. À exécuter
    /// dans la transaction ouverte par l'appelant.
    async fn delete_locked(&self, source_uid: &str) -> Result<bool> {
        let row = self
            .conn
            .query_opt(
                "SELECT decision_id FROM decision_sources WHERE source_uid = $1",
                &[&source_uid],
            )
            .await?;
        let Some(row) = row else {
            // Provenance inconnue : rien à retirer (re-delete / source jamais vue).
            return Ok(false);
        };
        let decision_id: i64 = row.get(0);
        // Verrou par décision (auto-relâché en fin de transaction) : sérialise
        // retrait et upsert concurrents sur la même décision (§4).
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

    /// `reconcile(decision_id)` (ADR 0098 §4) — recalcule la décision canonique
    /// comme **fonction pure de ses `decision_sources`**. À appeler après toute
    /// mutation de provenance (upsert ou tombstone), sous verrou, dans la même
    /// transaction.
    ///
    /// 1. **0 provenance** → hard-delete la décision (cascade chunks/refs).
    /// 2. **autorité** = provenance de `source_rank` max (**active ou
    ///    tombstonée**), départagée par rendition FR-prioritaire
    ///    (`lang = 'fra'` DESC, ADR 0127/0153) puis par id croissant.
    /// 3. **autorité active** → la décision est active : `deleted_at = NULL`. Le
    ///    `full_text` (texte de l'autorité) est déjà posé par l'ingest de cette
    ///    provenance — `reconcile` ne re-dérive **jamais** le texte (§2/§4.4 :
    ///    texte canonique unique, jamais par provenance).
    /// 4. **autorité tombstonée** → **vidé** (§5, RGPD) : `full_text`/`summary`
    ///    NULL, `deleted_at = NOW()`, chunks + payload brut + mots-clés supprimés.
    ///    On honore le retrait autoritaire **même si une source inférieure est
    ///    encore active** : pas de bascule sur un texte moins anonymisé.
    #[tracing::instrument(name = "db.reconcile", skip(self), fields(db.system = "postgresql"))]
    pub async fn reconcile(&self, decision_id: i64) -> Result<()> {
        // Autorité = rang max, active ou tombstonée. Son `deleted_at` décide de
        // l'état de la décision (§4.2-4.4).
        let authority = self
            .conn
            .query_opt(
                "SELECT deleted_at IS NULL AS active \
                 FROM decision_sources WHERE decision_id = $1 \
                 ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC LIMIT 1",
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
            // Autorité active → décision active. `full_text`/métadonnées déjà
            // posés par l'ingest de l'autorité ; on lève juste un éventuel vide.
            self.conn
                .execute(
                    "UPDATE decisions SET deleted_at = NULL \
                     WHERE id = $1 AND deleted_at IS NOT NULL",
                    &[&decision_id],
                )
                .await?;
            return Ok(());
        }

        // Autorité tombstonée → vidé (RGPD) : on retire tout porteur de données
        // perso (texte, chunks+embeddings, payload brut, résumé).
        self.conn
            .execute(
                "UPDATE decisions SET deleted_at = NOW(), full_text = NULL, summary = NULL \
                 WHERE id = $1",
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
                "DELETE FROM decision_full_text WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        Ok(())
    }

    /// Prédicat d'un tombstone **orphelin** (retrait total, purgeable) : décision
    /// retirée (`deleted_at`), vidée (`full_text IS NULL` — garde ceinture : on ne
    /// supprime jamais une ligne qui porte encore du texte), et **sans aucune
    /// provenance active**. Une provenance active la ressusciterait via
    /// [`reconcile`](Self::reconcile) (autorité rang max → texte re-servi) : ces
    /// tombstones multi-provenance sont donc **exclus**. Partagé par le comptage,
    /// l'échantillon et le DELETE pour garantir exactement la même cible.
    const ORPHAN_TOMBSTONE_WHERE: &'static str = "d.deleted_at IS NOT NULL \
         AND d.full_text IS NULL \
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

    /// Hard-delete des tombstones orphelins (cascade FK : provenances, chunks,
    /// `decision_full_text`, citations). Renvoie le nombre supprimé. La clause
    /// re-vérifie l'absence de provenance active **au moment du DELETE**
    /// (atomique, race-safe : une réactivation concurrente sort la ligne de la
    /// cible). Ne ressuscite jamais rien — c'est un retrait, pas une bascule.
    pub async fn purge_orphan_tombstones(&self) -> Result<u64> {
        let sql = format!(
            "DELETE FROM decisions d WHERE {}",
            Self::ORPHAN_TOMBSTONE_WHERE
        );
        Ok(self.conn.execute(&sql, &[]).await?)
    }

    /// Détache un groupe de provenances faussement fusionnées du canonique vers une
    /// **nouvelle** décision (#29 / ADR 0100 §5) — l'inverse d'une fusion.
    /// Sous la transaction de l'appelant : (1) INSERT d'une ligne `decisions`
    /// squelette (identité posée via `canonical_ref` du groupe — load-bearing pour
    /// la reprise même si le re-fetch échoue ; contenu + `ecli` remplis ensuite par
    /// la ré-ingestion du re-fetch) ; (2) re-pointe les `decision_sources` du groupe
    /// vers elle. Renvoie l'id créé. **Idempotent** : si les provenances du groupe
    /// ne sont plus sur `canonical_id` (déjà scindé), l'UPDATE ne touche 0 ligne et
    /// l'appelant détecte la reprise en amont (le cluster n'apparaît plus comme
    /// multi-provenance divergente).
    pub async fn create_split_decision(
        &self,
        canonical_id: i64,
        group_source_uids: &[String],
        jurisdiction_type: &str,
        public_id: &str,
        canonical_ref: &str,
    ) -> Result<i64> {
        // 1. Squelette : identité posée, contenu (full_text/chunks/embeddings)
        //    laissé vide — rempli par la ré-ingestion du re-fetch (update_existing).
        let new_id: i64 = self
            .conn
            .query_one(
                "INSERT INTO decisions (jurisdiction_type, public_id, updated_at, \
                   canonical_ref) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
                &[&jurisdiction_type, &public_id, &now(), &canonical_ref],
            )
            .await?
            .get(0);
        // 2. Re-pointe le groupe : SEULEMENT les provenances encore sur le
        //    canonique (idempotence : un groupe déjà déplacé ne rebouge pas).
        self.conn
            .execute(
                "UPDATE decision_sources SET decision_id = $1 \
                 WHERE source_uid = ANY($2) AND decision_id = $3",
                &[&new_id, &group_source_uids, &canonical_id],
            )
            .await?;
        Ok(new_id)
    }

    /// Clusters de décisions actives à **fusionner** (faux splits cross-source,
    /// ADR 0098/0100/0106) : même `canonical_ref`, ≥2 décisions actives, et
    /// **sources disjointes** (aucune source portée par 2 décisions du cluster).
    /// La disjonction garantit une fusion **sûre** : jamais deux provenances
    /// same-source recollées (invariant ADR 0104, perte de texte irréversible) —
    /// seules les affaires sérielles same-source sont ainsi **exclues** (un même
    /// `canonical_ref` réparti sur 2 décisions d'une même source reste scindé).
    /// Renvoie, par cluster, `(decision_id, max_source_rank)` triés rang décroissant
    /// puis id croissant : l'appelant garde `[0]` (autorité, son `full_text` est
    /// déjà canonique) et fusionne les suivants dedans.
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

    /// `source_uid` opendata des décisions dont l'autorité a basculé sur opendata
    /// après le passage de `source_rank` en colonne générée (ADR 0113), mais dont
    /// le `full_text` reste figé sur l'**autre** provenance (rang 50 :
    /// jade/constit/cedh/cjue/cnda) qui gagnait quand opendata valait 40. Cible
    /// exacte du re-ingest opendata ciblé (`reingest_stale_opendata`) : ces
    /// décisions (et elles seules) doivent re-parser leur payload opendata pour que
    /// `full_text`/chunks/embeddings repassent sur le texte opendata canonique.
    /// Exclut les décisions portant une provenance **judilibre** (rang 60 > 55 :
    /// judilibre reste autorité, aucun flip). `reconcile` ne réécrit pas le texte →
    /// seul un re-ingest de l'autorité le fait.
    #[tracing::instrument(name = "db.opendata_source_uids_stale_authority", skip(self), fields(db.system = "postgresql"))]
    pub async fn opendata_source_uids_stale_authority(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "
                SELECT od.source_uid
                FROM decision_sources od
                WHERE od.source = 'opendata' AND od.deleted_at IS NULL
                  AND EXISTS (
                      SELECT 1 FROM decision_sources o
                      WHERE o.decision_id = od.decision_id
                        AND o.source <> 'opendata' AND o.source_rank = 50
                        AND o.deleted_at IS NULL)
                  AND NOT EXISTS (
                      SELECT 1 FROM decision_sources j
                      WHERE j.decision_id = od.decision_id
                        AND j.source = 'judilibre' AND j.deleted_at IS NULL)
                ",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    /// Fusionne `loser_id` dans `keeper_id` — l'**inverse** de
    /// [`create_split_decision`](Self::create_split_decision). Re-pointe **toutes**
    /// les provenances du perdant (actives + tombstonées) vers le gardien, puis
    /// `reconcile` les deux : le perdant, désormais sans provenance, est
    /// hard-delete (cascade chunks/refs/full_text). Transactionnel, sous verrous
    /// `pg_advisory_xact_lock` ordonnés (anti-deadlock). **Sûr uniquement si les
    /// sources sont disjointes** (garanti par
    /// [`fetch_cross_source_merge_groups`](Self::fetch_cross_source_merge_groups)) :
    /// sinon le gardien porterait 2 provenances d'une même source (viole ADR 0098
    /// §4). Le `full_text`/chunks/embeddings du gardien (autorité de rang max)
    /// sont **conservés** (`reconcile` ne re-dérive jamais le texte) ; aucun
    /// re-embed. No-op si `keeper_id == loser_id`.
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

    /// Provenances judilibre **perdantes** d'un faux merge intra-source (#46 /
    /// ADR 0104) : pour chaque décision portant ≥2 provenances judilibre actives,
    /// renvoie celles de rang non-autoritaire (`row_number > 1` sur
    /// `source_rank DESC, id ASC` — l'autorité `rn=1` reste sur la décision
    /// d'origine, son texte y est déjà). Renvoie `(decision_id, source_uid,
    /// source_fields)` triées par `decision_id` (reprise stable). Le `source_fields`
    /// porte `jurisdiction` + `decision_date` → localisation du payload dans le
    /// cache local (`<jur>/<AAAAMM>.jsonl.gz`). Les provenances **tombstonées**
    /// (`deleted_at` non NULL) sont exclues : on ne re-matérialise jamais ce que
    /// judilibre a supprimé (RGPD).
    pub async fn fetch_same_source_judilibre_losers(&self) -> Result<Vec<(i64, String, Value)>> {
        let rows = self
            .conn
            .query(
                "WITH ranked AS ( \
                   SELECT s.decision_id, s.source_uid, s.source_fields, \
                          row_number() OVER (PARTITION BY s.decision_id \
                                             ORDER BY s.source_rank DESC, s.id ASC) AS rn \
                   FROM decision_sources s \
                   WHERE s.deleted_at IS NULL AND s.source = 'judilibre' \
                     AND s.decision_id IN ( \
                       SELECT decision_id FROM decision_sources \
                       WHERE deleted_at IS NULL AND source = 'judilibre' \
                       GROUP BY decision_id HAVING count(*) >= 2) \
                 ) \
                 SELECT decision_id, source_uid, source_fields FROM ranked \
                 WHERE rn > 1 ORDER BY decision_id",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Value>(2),
                )
            })
            .collect())
    }

    /// Provenances **perdantes** d'un faux merge intra-source `dila-jade` (#47 /
    /// ADR 0104, analogue de [`Self::fetch_same_source_judilibre_losers`]). Pour
    /// chaque décision portant ≥2 provenances `dila-jade` actives, renvoie celles
    /// de rang non-autoritaire (`row_number > 1`). Contrairement à judilibre (où le
    /// cache mensuel est indexé par `decision_date`), la re-matérialisation DILA
    /// passe par un **streaming des tarballs** (non indexés par décision) → on n'a
    /// pas besoin du `source_fields` ; on renvoie directement `canonical_ref` et
    /// `jurisdiction_type` **de la décision** (identiques au perdant, fusionnés sur
    /// la clé) pour amorcer le squelette de `create_split_decision` (la ré-ingestion
    /// du membre tar écrase ensuite ces champs). Tombstonés exclus (RGPD).
    pub async fn fetch_same_source_dila_jade_losers(
        &self,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "WITH ranked AS ( \
                   SELECT s.decision_id, s.source_uid, \
                          row_number() OVER (PARTITION BY s.decision_id \
                                             ORDER BY s.source_rank DESC, s.id ASC) AS rn \
                   FROM decision_sources s \
                   WHERE s.deleted_at IS NULL AND s.source = 'dila-jade' \
                     AND s.decision_id IN ( \
                       SELECT decision_id FROM decision_sources \
                       WHERE deleted_at IS NULL AND source = 'dila-jade' \
                       GROUP BY decision_id HAVING count(*) >= 2) \
                 ) \
                 SELECT r.decision_id, r.source_uid, d.canonical_ref, d.jurisdiction_type \
                 FROM ranked r JOIN decisions d ON d.id = r.decision_id \
                 WHERE r.rn > 1 ORDER BY r.decision_id",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, Option<String>>(2),
                    r.get::<_, Option<String>>(3),
                )
            })
            .collect())
    }

    /// Index de rattachement ArianeWeb (ADR 0204) : une entrée par
    /// (n° de dossier, date de lecture ISO) des décisions CE actives —
    /// `docket_numbers` dénesté. Chargé une fois par run `sync-ariane`, la
    /// jointure se fait en mémoire (192 k lignes CE, l'ECLI brut ne joint pas).
    #[tracing::instrument(name = "db.ce_docket_date_index", skip(self), fields(db.system = "postgresql"))]
    pub async fn ce_docket_date_index(&self) -> Result<Vec<(String, String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT unnest(docket_numbers), date_lecture::text, id \
                 FROM decisions \
                 WHERE jurisdiction_type = 'CE' AND deleted_at IS NULL \
                   AND date_lecture IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<_, String>(0),
                    r.get::<_, String>(1),
                    r.get::<_, i64>(2),
                )
            })
            .collect())
    }

    /// Checksums des bundles ArianeWeb déjà en base (`source = 'ariane-web'`),
    /// par `source_uid` — skip d'idempotence (#7) avant upsert.
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

    /// Upsert d'un commentaire de norme (ADR 0212). `num_key` NULL = commentaire
    /// du texte entier. Conflit sur `source_uid` (idempotent, #7).
    #[tracing::instrument(name = "db.upsert_article_commentaire", skip(self, source_fields), fields(db.system = "postgresql"))]
    pub async fn upsert_article_commentaire(
        &self,
        text_uid: &str,
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
                  (text_uid, num_key, source, source_uid, content_checksum, source_fields)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (source_uid) DO UPDATE SET
                  text_uid = EXCLUDED.text_uid,
                  num_key = EXCLUDED.num_key,
                  source = EXCLUDED.source,
                  content_checksum = EXCLUDED.content_checksum,
                  source_fields = EXCLUDED.source_fields,
                  deleted_at = NULL,
                  ingested_at = NOW()
                ",
                &[
                    &text_uid,
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

    /// Checksums des commentaires de norme d'une `source`, par `source_uid` —
    /// skip d'idempotence avant upsert.
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

    /// Commentaires d'un article de norme (page `/texte/{code}/{num_key}`) :
    /// entrées `commentaires[]` ancrées sur cet article **et** celles du texte
    /// entier (`num_key IS NULL`, ex. débats de la loi). Agrégées en un tableau
    /// jsonb, `None` si aucune. La propagation depuis les textes modificateurs
    /// (`legal_link`) est un enrichissement ultérieur.
    #[tracing::instrument(name = "db.article_commentaires", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_commentaires(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Option<Value>> {
        let row = self
            .conn
            .query_opt(
                "SELECT jsonb_agg(c) FROM article_commentaire ac \
                 CROSS JOIN LATERAL jsonb_array_elements(ac.source_fields->'commentaires') c \
                 WHERE ac.text_uid = $1 AND (ac.num_key = $2 OR ac.num_key IS NULL) \
                   AND ac.deleted_at IS NULL \
                   AND jsonb_typeof(ac.source_fields->'commentaires') = 'array'",
                &[&text_uid, &num_key],
            )
            .await?;
        Ok(row.and_then(|r| r.get::<_, Option<Value>>(0)))
    }

    /// Résout une décision par (n° de dossier, date de lecture) — le n° seul
    /// n'est pas unique (un `2509454` de TA existe à plusieurs dates). Renvoie
    /// les `(id, public_id)` correspondants (plusieurs si des dossiers homonymes
    /// partagent la date, cas rare — le `public_id` distingue alors les
    /// `source_uid`). Utilisé pour rattacher les commentaires doctrine web.
    #[tracing::instrument(name = "db.decisions_by_docket_date", skip(self), fields(db.system = "postgresql"))]
    pub async fn decisions_by_docket_date(
        &self,
        docket: &str,
        date_iso: &str,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, public_id FROM decisions \
                 WHERE $1 = ANY(docket_numbers) AND date_lecture::text = $2 \
                   AND deleted_at IS NULL",
                &[&docket, &date_iso],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Checksums des commentaires ADDE déjà en base (`source = 'adde'`), par
    /// `source_uid` — skip d'idempotence (#7) avant upsert.
    #[tracing::instrument(name = "db.adde_checksums", skip(self), fields(db.system = "postgresql"))]
    pub async fn adde_checksums(&self) -> Result<HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, content_checksum FROM decision_sources \
                 WHERE source = 'adde' AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect())
    }
}
