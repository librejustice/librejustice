//! Référentiel d'entités : chargement des registres externes (SIRENE, RNA…)
//! par remplacement de namespace + COPY binaire.
//!
//! L'appelant (lj-ingest) streame la source et pousse des lots ; le
//! remplacement est encadré par `entity_namespace_clear` (DELETE du
//! namespace) et les lots par `entity_copy` — le tout dans la transaction que
//! possède l'appelant.

use super::types::{
    DecisionPartyReadRow, EntityContentieuxCounts, EntityDecisionRow, EntityDenominationReadRow,
    EntityDirectoryRow, EntityHeaderRow, EntityJurisdictionCountRow, EntityYearCountRow,
};
use super::DecisionRepository;
use crate::error::Result;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Une entité de registre prête à charger. Aucun folded : le pliage vit dans
/// les index d'expression `lj_fold`/`lj_fold_all`, jamais en heap.
#[derive(Debug, Clone)]
pub struct EntityWriteItem {
    /// Uid namespacé (`siren:552081317`, `rna:W123456789`).
    pub uid: String,
    /// `morale_privee` | `morale_publique` | `physique`.
    pub nature: &'static str,
    pub denomination: String,
    pub sigle: Option<String>,
    /// Catégorie juridique source, brute (ex. `5710` Insee).
    pub forme: Option<String>,
    pub active: bool,
    /// Patronyme composé plié, tirets normalisés en espaces (avocats CNB) — clé
    /// du sous-étage de résolution nom-seul.
    pub surname_key: Option<String>,
    /// Catégorie d'annuaire — dérivée par le chargeur, écrivain unique de la
    /// règle (namespace × nature × APE).
    pub category: &'static str,
    /// Code APE Insee brut (`69.10Z`, `siren:` seulement).
    pub ape: Option<String>,
    /// Slug barreau (`cnb:` seulement, 2ᵉ segment de l'uid).
    pub barreau: Option<String>,
    /// Dénominations supplémentaires du jsonb `denominations` (ordre inverse
    /// `NOM PRENOM` et nom commercial des EI) — surface de résolution GIN, sans
    /// date.
    pub alt_denominations: Vec<String>,
    /// Évidences d'activité datées `[{du?, au?, src}]` — consommées par
    /// `lj_active_at` à la résolution. `None` = pas d'évidence (registres non
    /// datés : SIRENE porte ses périodes dans `denominations`, RNA rien).
    pub activity: Option<serde_json::Value>,
}

/// État courant d'une entité `cnb:` lu pour le diff d'upsert du reload.
#[derive(Debug)]
pub struct CnbEntityState {
    pub uid: String,
    pub active: bool,
    pub activity: Option<serde_json::Value>,
    pub barreau: Option<String>,
    pub denomination: String,
    pub surname_key: Option<String>,
    pub forme: Option<String>,
}

/// Mise à jour d'une entité `cnb:` existante (upsert du reload).
#[derive(Debug)]
pub struct CnbEntityUpdate {
    pub uid: String,
    pub active: bool,
    pub activity: Option<serde_json::Value>,
    pub barreau: Option<String>,
    pub denomination: String,
    pub forme: Option<String>,
    pub surname_key: Option<String>,
}

/// Arête d'affiliation avocat ↔ structure à écrire. Chaque borne absente est
/// une borne INCONNUE, pas une borne infinie : `au` vide est l'état courant tant
/// qu'un reload ne l'a pas fermée.
#[derive(Debug, Clone)]
pub struct AffiliationEdge {
    pub entity_uid: String,
    pub structure_uid: String,
    pub du: Option<chrono::NaiveDate>,
    pub au: Option<chrono::NaiveDate>,
}

/// Une dénomination historique datée (période close, hors nom courant) —
/// stagée puis fusionnée dans `entity.denominations`. Pas de folded :
/// lj_fold_all le calcule à l'indexation.
#[derive(Debug, Clone)]
pub struct EntityHistoryWriteItem {
    pub entity_uid: String,
    pub denomination: String,
    pub date_debut: Option<chrono::NaiveDate>,
    pub date_fin: Option<chrono::NaiveDate>,
}

impl DecisionRepository<'_> {
    /// Vide un namespace (`siren`, `rna`…) avant rechargement complet —
    /// idempotence par remplacement. À appeler dans la transaction du chargeur ;
    /// lève le `statement_timeout` du pool pour le reste de cette transaction (le
    /// DELETE de masse entretient les index annuaire, au-delà des 30 s).
    pub async fn entity_namespace_clear(&self, namespace: &str) -> Result<u64> {
        self.conn
            .batch_execute("SET LOCAL statement_timeout = 0")
            .await?;
        let pattern = format!("{namespace}:%");
        let n = self
            .conn
            .execute("DELETE FROM entity WHERE uid LIKE $1", &[&pattern])
            .await?;
        Ok(n)
    }

    /// COPY binaire d'un lot d'entités. Les uids d'un run sont uniques
    /// (source = stock dédupliqué) — violation de PK = bug amont, erreur
    /// franche. `denominations` part avec le seul nom courant `[{"d": …}]`
    /// (écrivain unique de la forme) ; l'historique arrive par
    /// `entity_history_merge`.
    pub async fn entity_copy(&self, items: &[EntityWriteItem]) -> Result<()> {
        let sink = self
            .conn
            .copy_in(
                "COPY entity (uid, nature, denomination, denominations, \
                 sigle, forme, active, surname_key, category, ape, barreau, \
                 activity) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::JSONB,
                Type::TEXT,
                Type::TEXT,
                Type::BOOL,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::JSONB,
            ],
        );
        tokio::pin!(writer);
        for it in items {
            let denominations = serde_json::Value::Array(
                std::iter::once(&it.denomination)
                    .chain(it.alt_denominations.iter())
                    .map(|d| serde_json::json!({ "d": d }))
                    .collect(),
            );
            writer
                .as_mut()
                .write(&[
                    &it.uid,
                    &it.nature,
                    &it.denomination,
                    &denominations,
                    &it.sigle,
                    &it.forme,
                    &it.active,
                    &it.surname_key,
                    &it.category,
                    &it.ape,
                    &it.barreau,
                    &it.activity,
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
    }

    /// État courant du namespace `cnb:` pour le diff d'upsert.
    pub async fn entity_cnb_states(&self) -> Result<Vec<CnbEntityState>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, active, activity, barreau, denomination, surname_key, forme \
                 FROM entity WHERE uid LIKE 'cnb:%'",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CnbEntityState {
                uid: r.get(0),
                active: r.get(1),
                activity: r.get(2),
                barreau: r.get(3),
                denomination: r.get(4),
                surname_key: r.get(5),
                forme: r.get(6),
            })
            .collect())
    }

    /// Mise à jour bulk d'entités `cnb:` existantes (upsert du reload) :
    /// active/activity/barreau/denomination/forme/surname_key. `denominations` est
    /// reconstruit depuis la dénomination courante (écrivain unique de la forme,
    /// comme `entity_copy`).
    pub async fn entity_cnb_update_bulk(&self, items: &[CnbEntityUpdate]) -> Result<u64> {
        if items.is_empty() {
            return Ok(0);
        }
        let uids: Vec<&str> = items.iter().map(|i| i.uid.as_str()).collect();
        let actives: Vec<bool> = items.iter().map(|i| i.active).collect();
        let activities: Vec<Option<&serde_json::Value>> =
            items.iter().map(|i| i.activity.as_ref()).collect();
        let barreaux: Vec<Option<&str>> = items.iter().map(|i| i.barreau.as_deref()).collect();
        let denoms: Vec<&str> = items.iter().map(|i| i.denomination.as_str()).collect();
        let formes: Vec<Option<&str>> = items.iter().map(|i| i.forme.as_deref()).collect();
        let surnames: Vec<Option<&str>> = items.iter().map(|i| i.surname_key.as_deref()).collect();
        let n = self
            .conn
            .execute(
                "UPDATE entity e SET \
                     active = u.active, activity = u.activity, barreau = u.barreau, \
                     denomination = u.denomination, \
                     denominations = jsonb_build_array(jsonb_build_object('d', u.denomination)), \
                     forme = u.forme, surname_key = u.surname_key \
                 FROM unnest($1::text[], $2::bool[], $3::jsonb[], $4::text[], \
                             $5::text[], $6::text[], $7::text[]) \
                      AS u(uid, active, activity, barreau, denomination, forme, surname_key) \
                 WHERE e.uid = u.uid",
                &[
                    &uids,
                    &actives,
                    &activities,
                    &barreaux,
                    &denoms,
                    &formes,
                    &surnames,
                ],
            )
            .await?;
        Ok(n)
    }

    // ── Affiliations avocat ↔ structure ───────────────────────────────────────

    /// Arêtes OUVERTES (`au IS NULL`) d'une source — l'état courant que le
    /// reload diffe. Renvoie `(entity_uid, structure_uid, du)`.
    pub async fn affiliation_open_edges(
        &self,
        source: &str,
    ) -> Result<Vec<(String, String, Option<chrono::NaiveDate>)>> {
        let rows = self
            .conn
            .query(
                "SELECT entity_uid, structure_uid, du FROM entity_affiliation \
                 WHERE source = $1 AND au IS NULL",
                &[&source],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Ferme des arêtes ouvertes (`au = date`) — départs et changements de
    /// structure observés par le reload.
    pub async fn affiliation_close_bulk(
        &self,
        source: &str,
        entity_uids: &[String],
        au: chrono::NaiveDate,
    ) -> Result<u64> {
        if entity_uids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "UPDATE entity_affiliation SET au = $3 \
                 WHERE source = $1 AND entity_uid = ANY($2) AND au IS NULL",
                &[&source, &entity_uids, &au],
            )
            .await?;
        Ok(n)
    }

    /// Insère des arêtes. Borne absente = inconnue : `au` vide laisse l'arête
    /// ouverte (état courant d'un reload), `au` posé écrit une période close
    /// (période observée par un rejeu de snapshots).
    pub async fn affiliation_insert_bulk(
        &self,
        source: &str,
        rows: &[AffiliationEdge],
    ) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let entity_uids: Vec<&str> = rows.iter().map(|r| r.entity_uid.as_str()).collect();
        let structure_uids: Vec<&str> = rows.iter().map(|r| r.structure_uid.as_str()).collect();
        let dus: Vec<Option<chrono::NaiveDate>> = rows.iter().map(|r| r.du).collect();
        let aus: Vec<Option<chrono::NaiveDate>> = rows.iter().map(|r| r.au).collect();
        let n = self
            .conn
            .execute(
                "INSERT INTO entity_affiliation (entity_uid, structure_uid, du, au, source) \
                 SELECT u.entity_uid, u.structure_uid, u.du, u.au, $5 \
                 FROM unnest($1::text[], $2::text[], $3::date[], $4::date[]) \
                      AS u(entity_uid, structure_uid, du, au)",
                &[&entity_uids, &structure_uids, &dus, &aus, &source],
            )
            .await?;
        Ok(n)
    }

    /// Supprime toutes les arêtes d'une source — un rejeu recalcule ses
    /// évidences en entier depuis les snapshots, il les remplace.
    pub async fn affiliation_source_clear(&self, source: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM entity_affiliation WHERE source = $1",
                &[&source],
            )
            .await?;
        Ok(n)
    }

    /// Arêtes dérivées des appositions des décisions : un conseil résolu vers
    /// `cnb:` et le cabinet apposé résolu par `firm_ord`, attestés par au moins
    /// `support` décisions DISTINCTES. Périodes CLOSES bornées par les dates de
    /// lecture du support : l'index unique partiel n'admet qu'une arête ouverte par
    /// source, or un avocat plaide légitimement pour deux structures sur la
    /// période.
    pub async fn affiliation_from_decisions(
        &self,
        support: i64,
    ) -> Result<Vec<(AffiliationEdge, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT a.entity_uid, x.actors -> a.firm_ord ->> 'e', \
                        min(d.date_lecture), max(d.date_lecture), count(DISTINCT x.decision_id) \
                 FROM decision_actors x \
                 CROSS JOIN LATERAL public.lj_actor_rows(x.actors) a \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE a.firm_ord IS NOT NULL \
                   AND a.entity_uid LIKE 'cnb:%' \
                   AND x.actors -> a.firm_ord ? 'e' \
                   AND d.date_lecture IS NOT NULL \
                 GROUP BY 1, 2 \
                 HAVING count(DISTINCT x.decision_id) >= $1",
                &[&support],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    AffiliationEdge {
                        entity_uid: r.get(0),
                        structure_uid: r.get(1),
                        du: r.get(2),
                        au: r.get(3),
                    },
                    r.get(4),
                )
            })
            .collect())
    }

    // ── Re-key CNBF (one-shot) ────────────────────────────────────────────────

    /// The `cnb:` uids actors link to with no entity left: the dangling ones of
    /// the clear+copy reload (departures and barreau changes).
    pub async fn dangling_cnb_actor_uids(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT a.entity_uid FROM decision_actor a \
                 WHERE a.entity_uid LIKE 'cnb:%' \
                   AND NOT EXISTS (SELECT 1 FROM entity e WHERE e.uid = a.entity_uid)",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Renomme des uids d'entités (le re-key garde la ligne : compteurs,
    /// dénominations et liens par valeur suivent le rename des liens).
    pub async fn entity_rename_bulk(&self, pairs: &[(String, String)]) -> Result<u64> {
        if pairs.is_empty() {
            return Ok(0);
        }
        let olds: Vec<&str> = pairs.iter().map(|p| p.0.as_str()).collect();
        let news: Vec<&str> = pairs.iter().map(|p| p.1.as_str()).collect();
        let n = self
            .conn
            .execute(
                "UPDATE entity e SET uid = u.new_uid \
                 FROM unnest($1::text[], $2::text[]) AS u(old_uid, new_uid) \
                 WHERE e.uid = u.old_uid",
                &[&olds, &news],
            )
            .await?;
        Ok(n)
    }

    /// Supprime des entités par uid (pieds de fichier, doublons fusionnés).
    pub async fn entity_delete_bulk(&self, uids: &[String]) -> Result<u64> {
        if uids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute("DELETE FROM entity WHERE uid = ANY($1)", &[&uids])
            .await?;
        Ok(n)
    }

    /// Moves the actors linked to `old` onto `new`, in every document. Returns
    /// the documents rewritten, and their decisions when `returning_decisions`
    /// (merges and dangling uids: their targets gain links, which
    /// `refresh_annuaire_scoped` must recount).
    pub async fn actor_remap_entities(
        &self,
        pairs: &[(String, String)],
        returning_decisions: bool,
    ) -> Result<(u64, Vec<i64>)> {
        if pairs.is_empty() {
            return Ok((0, Vec::new()));
        }
        let olds: Vec<&str> = pairs.iter().map(|p| p.0.as_str()).collect();
        let news: Vec<&str> = pairs.iter().map(|p| p.1.as_str()).collect();
        let rows = self
            .conn
            .query(
                "UPDATE decision_actors x \
                 SET actors = public.lj_actor_remap_entities(x.actors, $1, $2) \
                 WHERE public.lj_actor_terms(x.actors) \
                       && ARRAY(SELECT 'e|' || o FROM unnest($1::text[]) o) \
                 RETURNING x.decision_id",
                &[&olds, &news],
            )
            .await?;
        let mut ids: Vec<i64> = if returning_decisions {
            rows.iter().map(|r| r.get(0)).collect()
        } else {
            Vec::new()
        };
        ids.sort_unstable();
        Ok((rows.len() as u64, ids))
    }

    /// Unlinks the actors linked to the given uids (purge of bogus ones).
    pub async fn actor_unlink_uids(&self, uids: &[String]) -> Result<u64> {
        if uids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "UPDATE decision_actors x \
                 SET actors = public.lj_actor_remap_entities( \
                     x.actors, $1, array_fill(NULL::text, ARRAY[cardinality($1::text[])])) \
                 WHERE public.lj_actor_terms(x.actors) \
                       && ARRAY(SELECT 'e|' || u FROM unnest($1::text[]) u)",
                &[&uids],
            )
            .await?;
        Ok(n)
    }

    /// Backfill du `du` d'arêtes ouvertes qui n'en avaient pas quand la source se
    /// met à le déclarer (`acDateEntree` CNB).
    pub async fn affiliation_backfill_du(
        &self,
        source: &str,
        rows: &[(String, chrono::NaiveDate)],
    ) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let entity_uids: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let dus: Vec<chrono::NaiveDate> = rows.iter().map(|r| r.1).collect();
        let n = self
            .conn
            .execute(
                "UPDATE entity_affiliation a SET du = u.du \
                 FROM unnest($1::text[], $2::date[]) AS u(entity_uid, du) \
                 WHERE a.source = $3 AND a.entity_uid = u.entity_uid \
                   AND a.au IS NULL AND a.du IS NULL",
                &[&entity_uids, &dus, &source],
            )
            .await?;
        Ok(n)
    }

    /// Table de staging (temporaire, transaction du chargeur) des
    /// dénominations historiques — fusionnées d'un bloc par
    /// `entity_history_merge` en fin de chargement.
    pub async fn entity_history_stage_init(&self) -> Result<()> {
        self.conn
            .batch_execute(
                "CREATE TEMP TABLE entity_history_stage ( \
                 entity_uid text NOT NULL, denomination text NOT NULL, \
                 date_debut date, date_fin date) ON COMMIT DROP",
            )
            .await?;
        Ok(())
    }

    /// COPY binaire d'un lot de dénominations historiques vers le staging.
    pub async fn entity_history_stage_copy(&self, items: &[EntityHistoryWriteItem]) -> Result<()> {
        let sink = self
            .conn
            .copy_in(
                "COPY entity_history_stage (entity_uid, denomination, \
                 date_debut, date_fin) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer =
            BinaryCopyInWriter::new(sink, &[Type::TEXT, Type::TEXT, Type::DATE, Type::DATE]);
        tokio::pin!(writer);
        for it in items {
            writer
                .as_mut()
                .write(&[
                    &it.entity_uid,
                    &it.denomination,
                    &it.date_debut,
                    &it.date_fin,
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
    }

    /// Fusionne le staging dans `entity.denominations` : union nom(s) en
    /// place + historique, dédupliquée par objet (l'historique SIRENE porte
    /// des périodes dupliquées). Renvoie le nombre d'entités enrichies.
    pub async fn entity_history_merge(&self) -> Result<u64> {
        self.conn
            .batch_execute("CREATE INDEX ON entity_history_stage (entity_uid)")
            .await?;
        let n = self
            .conn
            .execute(
                "UPDATE entity e SET denominations = ( \
                     SELECT jsonb_agg(DISTINCT el) FROM ( \
                         SELECT jsonb_array_elements(e.denominations) AS el \
                         UNION ALL \
                         SELECT jsonb_strip_nulls(jsonb_build_object( \
                                    'd', s.denomination, \
                                    'du', s.date_debut::text, \
                                    'au', s.date_fin::text)) \
                         FROM entity_history_stage s WHERE s.entity_uid = e.uid \
                     ) u \
                 ) \
                 WHERE e.uid IN (SELECT DISTINCT entity_uid FROM entity_history_stage)",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Volumétrie par namespace (rapport de fin de chargement).
    pub async fn entity_count(&self, namespace: &str) -> Result<i64> {
        let pattern = format!("{namespace}:%");
        let row = self
            .conn
            .query_one("SELECT count(*) FROM entity WHERE uid LIKE $1", &[&pattern])
            .await?;
        Ok(row.get(0))
    }

    // ── Fiche entité (lecture) ────────────────────────────────────────────────

    /// En-tête d'une entité par uid (`entity`). `None` = uid inconnu (→ 404).
    pub async fn entity_header(&self, uid: &str) -> Result<Option<EntityHeaderRow>> {
        let row = self
            .conn
            .query_opt(
                "SELECT uid, nature, denomination, sigle, forme, active \
                 FROM entity WHERE uid = $1",
                &[&uid],
            )
            .await?;
        Ok(row.map(|r| EntityHeaderRow {
            uid: r.get(0),
            nature: r.get(1),
            denomination: r.get(2),
            sigle: r.get(3),
            forme: r.get(4),
            active: r.get(5),
        }))
    }

    /// Dénominations datées d'une entité (la courante incluse), ordre
    /// chronologique — période ouverte (`du` absent) d'abord. Lues du jsonb
    /// `entity.denominations` ; tri textuel = chronologique (dates ISO).
    pub async fn entity_denominations(&self, uid: &str) -> Result<Vec<EntityDenominationReadRow>> {
        let rows = self
            .conn
            .query(
                "SELECT el->>'d', el->>'du', el->>'au' \
                 FROM entity, jsonb_array_elements(denominations) AS el \
                 WHERE uid = $1 \
                 ORDER BY el->>'du' ASC NULLS FIRST, el->>'au' ASC NULLS LAST",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| EntityDenominationReadRow {
                denomination: r.get(0),
                date_debut: r.get(1),
                date_fin: r.get(2),
            })
            .collect())
    }

    /// Comptes contentieux (décisions distinctes, non soft-deleted) : total +
    /// répartition par côté, en une requête.
    pub async fn entity_contentieux_counts(&self, uid: &str) -> Result<EntityContentieuxCounts> {
        let row = self
            .conn
            .query_one(
                "SELECT count(DISTINCT x.decision_id), \
                        count(DISTINCT x.decision_id) FILTER (WHERE p.side = 'applicant'), \
                        count(DISTINCT x.decision_id) FILTER (WHERE p.side = 'defendant') \
                 FROM decision_actors x \
                 CROSS JOIN LATERAL public.lj_actor_rows(x.actors) p \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE public.lj_actor_terms(x.actors) @> ARRAY['e|' || $1] \
                   AND p.entity_uid = $1 AND d.deleted_at IS NULL",
                &[&uid],
            )
            .await?;
        Ok(EntityContentieuxCounts {
            decision_count: row.get(0),
            as_applicant: row.get(1),
            as_defendant: row.get(2),
        })
    }

    /// Décisions de l'entité par année de lecture, ordre chronologique.
    pub async fn entity_by_year(&self, uid: &str) -> Result<Vec<EntityYearCountRow>> {
        let rows = self
            .conn
            .query(
                "SELECT EXTRACT(YEAR FROM d.date_lecture)::int AS year, \
                        count(DISTINCT x.decision_id) \
                 FROM decision_actors x \
                 CROSS JOIN LATERAL public.lj_actor_rows(x.actors) p \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE public.lj_actor_terms(x.actors) @> ARRAY['e|' || $1] \
                   AND p.entity_uid = $1 AND d.deleted_at IS NULL \
                   AND d.date_lecture IS NOT NULL \
                 GROUP BY year ORDER BY year",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| EntityYearCountRow {
                year: r.get(0),
                count: r.get(1),
            })
            .collect())
    }

    /// Décisions de l'entité par juridiction, décroissant (libellé résolu côté API).
    pub async fn entity_by_jurisdiction(
        &self,
        uid: &str,
    ) -> Result<Vec<EntityJurisdictionCountRow>> {
        let rows = self
            .conn
            .query(
                "SELECT d.jurisdiction_code, d.jurisdiction_type, count(DISTINCT x.decision_id) AS n \
                 FROM decision_actors x \
                 CROSS JOIN LATERAL public.lj_actor_rows(x.actors) p \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE public.lj_actor_terms(x.actors) @> ARRAY['e|' || $1] \
                   AND p.entity_uid = $1 AND d.deleted_at IS NULL \
                 GROUP BY d.jurisdiction_code, d.jurisdiction_type \
                 ORDER BY n DESC, d.jurisdiction_type",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| EntityJurisdictionCountRow {
                jurisdiction_code: r.get(0),
                jurisdiction_type: r.get(1),
                count: r.get(2),
            })
            .collect())
    }

    /// Décisions citant l'entité (toutes qualités), plus récentes d'abord,
    /// paginées : ids internes destinés à l'hydratation `SearchHit` côté API
    /// (rendu unifié avec la recherche) + rôle représentatif de l'entité (pick
    /// stable `party` > `law_firm` > `counsel_name`). Renvoie `(total, page)`.
    pub async fn entity_decisions(
        &self,
        uid: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(i64, Vec<EntityDecisionRow>)> {
        let total: i64 = self
            .conn
            .query_one(
                "SELECT count(DISTINCT x.decision_id) \
                 FROM decision_actors x \
                 CROSS JOIN LATERAL public.lj_actor_rows(x.actors) p \
                 JOIN decisions d ON d.id = x.decision_id \
                 WHERE public.lj_actor_terms(x.actors) @> ARRAY['e|' || $1] \
                   AND p.entity_uid = $1 AND d.deleted_at IS NULL",
                &[&uid],
            )
            .await?
            .get(0);

        let rows = self
            .conn
            .query(
                "SELECT pr.decision_id, pr.quality, pr.side \
                 FROM ( \
                     SELECT DISTINCT ON (x.decision_id) x.decision_id, a.quality, a.side \
                     FROM decision_actors x \
                     CROSS JOIN LATERAL public.lj_actor_rows(x.actors) a \
                     WHERE public.lj_actor_terms(x.actors) @> ARRAY['e|' || $1] \
                       AND a.entity_uid = $1 \
                     ORDER BY x.decision_id, \
                       CASE a.quality WHEN 'party' THEN 0 WHEN 'law_firm' THEN 1 \
                                      WHEN 'counsel_name' THEN 2 ELSE 3 END, a.ord \
                 ) pr \
                 JOIN decisions d ON d.id = pr.decision_id \
                 WHERE d.deleted_at IS NULL \
                 ORDER BY d.date_lecture DESC NULLS LAST, d.public_id \
                 LIMIT $2 OFFSET $3",
                &[&uid, &limit, &offset],
            )
            .await?;
        let items = rows
            .iter()
            .map(|r| EntityDecisionRow {
                decision_id: r.get(0),
                quality: r.get(1),
                side: r.get(2),
            })
            .collect();
        Ok((total, items))
    }

    /// A decision's actors by `public_id`, in a stable order (`party` >
    /// `law_firm` > `counsel_name`, then `applicant` before `defendant`, then
    /// `ord`). `None` for an unknown decision (404), `Some(vec![])` for a known
    /// decision with no actor extracted.
    pub async fn decision_parties(
        &self,
        public_id: &str,
    ) -> Result<Option<Vec<DecisionPartyReadRow>>> {
        let exists = self
            .conn
            .query_opt(
                "SELECT 1 FROM decisions WHERE public_id = $1 AND deleted_at IS NULL",
                &[&public_id],
            )
            .await?;
        if exists.is_none() {
            return Ok(None);
        }
        let rows = self
            .conn
            .query(
                "SELECT p.quality, p.side, p.value, p.nature, p.barreau, p.entity_uid, \
                        p.ord, p.party_ord \
                 FROM decisions d \
                 JOIN decision_actors x ON x.decision_id = d.id \
                 CROSS JOIN LATERAL public.lj_actor_rows(x.actors) p \
                 WHERE d.public_id = $1 \
                 ORDER BY \
                   CASE p.quality WHEN 'party' THEN 0 WHEN 'law_firm' THEN 1 \
                                  WHEN 'counsel_name' THEN 2 ELSE 3 END, \
                   CASE p.side WHEN 'applicant' THEN 0 WHEN 'defendant' THEN 1 ELSE 2 END, \
                   p.ord",
                &[&public_id],
            )
            .await?;
        Ok(Some(
            rows.iter()
                .map(|r| DecisionPartyReadRow {
                    quality: r.get(0),
                    side: r.get(1),
                    value: r.get(2),
                    nature: r.get(3),
                    barreau: r.get(4),
                    entity_uid: r.get(5),
                    ord: r.get(6),
                    party_ord: r.get(7),
                })
                .collect(),
        ))
    }

    // ── Annuaire des entités ──────────────────────────────────────────────────

    /// Scoped directory refresh (end of a targeted `reextract-fields` run):
    /// recounts `decision_count` of the entities linked from the batch's
    /// decisions alone. Neither zeroes an entity unlinked everywhere nor reseeds
    /// `annuaire_registre`: a full `parties-relink` does the full table. Own
    /// transaction, timeout lifted. Returns the number of entities recounted.
    #[tracing::instrument(name = "db.refresh_annuaire_scoped", skip_all, fields(db.system = "postgresql", ids = decision_ids.len()))]
    pub async fn refresh_annuaire_scoped(&self, decision_ids: &[i64]) -> Result<u64> {
        if decision_ids.is_empty() {
            return Ok(0);
        }
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let n = self
                .conn
                .execute(
                    "WITH touched AS ( \
                         SELECT DISTINCT a.entity_uid AS uid \
                         FROM decision_actors x \
                         CROSS JOIN LATERAL public.lj_actor_rows(x.actors) a \
                         WHERE x.decision_id = ANY($1) AND a.entity_uid IS NOT NULL \
                     ), cnt AS ( \
                         SELECT t.uid, ( \
                             SELECT count(*) FROM decision_actors x \
                             JOIN decisions d ON d.id = x.decision_id \
                             WHERE public.lj_actor_terms(x.actors) @> ARRAY['e|' || t.uid] \
                               AND d.deleted_at IS NULL \
                         ) AS n \
                         FROM touched t \
                     ) \
                     UPDATE entity e \
                     SET decision_count = cnt.n \
                     FROM cnt \
                     WHERE e.uid = cnt.uid AND e.decision_count <> cnt.n",
                    &[&decision_ids],
                )
                .await?;
            Ok(n)
        }
        .await;
        match result {
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

    #[tracing::instrument(name = "db.refresh_annuaire", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_annuaire(&self) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            self.conn
                .batch_execute(
                    "CREATE TEMP TABLE lj_annuaire_counts ON COMMIT DROP AS \
                     SELECT a.entity_uid AS uid, count(DISTINCT a.decision_id) AS n \
                     FROM decision_actor a \
                     JOIN decisions d ON d.id = a.decision_id \
                     WHERE a.entity_uid IS NOT NULL AND d.deleted_at IS NULL \
                     GROUP BY a.entity_uid; \
                     ANALYZE lj_annuaire_counts; \
                     UPDATE entity e SET decision_count = c.n \
                     FROM lj_annuaire_counts c \
                     WHERE e.uid = c.uid AND e.decision_count <> c.n; \
                     UPDATE entity e SET decision_count = 0 \
                     WHERE e.decision_count > 0 \
                       AND NOT EXISTS (SELECT 1 FROM lj_annuaire_counts c WHERE c.uid = e.uid)",
                )
                .await?;
            self.conn
                .batch_execute("TRUNCATE annuaire_registre")
                .await?;
            self.conn
                .execute(
                    "INSERT INTO annuaire_registre (category, total, contentieux) \
                     SELECT category, count(*), \
                            count(*) FILTER (WHERE decision_count > 0) \
                     FROM entity GROUP BY category",
                    &[],
                )
                .await?;
            let row = self
                .conn
                .query_one(
                    "SELECT coalesce(sum(contentieux), 0)::bigint FROM annuaire_registre",
                    &[],
                )
                .await?;
            Ok(row.get::<_, i64>(0) as u64)
        }
        .await;
        match result {
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

    /// Prefix search over the whole registry, restricted to `categories` when
    /// given (`None` = every category). Caller owns the fold: `folded_like` is
    /// folded like `lj_fold`, LIKE wildcards escaped, `%` appended.
    ///
    /// `USING ~<~` is the C ordering of the text_pattern_ops opclass, the only one
    /// `entity_prefix_idx` can serve without a sort.
    pub async fn entity_search(
        &self,
        folded_like: &str,
        categories: Option<&[&str]>,
        limit: i64,
    ) -> Result<Vec<EntityDirectoryRow>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, namespace, denomination, nature, forme, active, \
                        barreau, decision_count \
                 FROM ( \
                     (SELECT uid, split_part(uid, ':', 1) AS namespace, denomination, \
                             nature, forme, active, barreau, decision_count, \
                             lj_fold(denomination) AS df, 0 AS leg \
                      FROM entity \
                      WHERE lj_fold(denomination) LIKE $1 ESCAPE '\\' \
                        AND ($2::text[] IS NULL OR category = ANY($2)) \
                        AND decision_count > 0 \
                      ORDER BY decision_count DESC, lj_fold(denomination) \
                      LIMIT $3) \
                     UNION ALL \
                     (SELECT uid, split_part(uid, ':', 1), denomination, \
                             nature, forme, active, barreau, decision_count, \
                             lj_fold(denomination), 1 \
                      FROM entity \
                      WHERE lj_fold(denomination) LIKE $1 ESCAPE '\\' \
                        AND ($2::text[] IS NULL OR category = ANY($2)) \
                        AND decision_count = 0 \
                      ORDER BY lj_fold(denomination) USING ~<~ \
                      LIMIT $3) \
                 ) t \
                 ORDER BY leg, decision_count DESC, df \
                 LIMIT $3",
                &[&folded_like, &categories, &limit],
            )
            .await?;
        Ok(rows.iter().map(directory_row).collect())
    }

    /// Paginated listing of one directory category, contentious volume first, then
    /// denomination. `total` (rows in the category) and `contentieux` (those with
    /// at least one decision) are read from `annuaire_registre`, refreshed at
    /// relink, so neither is counted live.
    pub async fn entity_directory(
        &self,
        category: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(i64, i64, Vec<EntityDirectoryRow>)> {
        let counts = self
            .conn
            .query_one(
                "SELECT coalesce((SELECT total FROM annuaire_registre \
                                  WHERE category = $1), 0), \
                        coalesce((SELECT contentieux FROM annuaire_registre \
                                  WHERE category = $1), 0)",
                &[&category],
            )
            .await?;
        let (total, contentieux): (i64, i64) = (counts.get(0), counts.get(1));
        let rows = self
            .conn
            .query(
                "SELECT uid, split_part(uid, ':', 1), denomination, nature, forme, \
                        active, barreau, decision_count \
                 FROM entity \
                 WHERE category = $1 \
                 ORDER BY decision_count DESC, lj_fold(denomination) \
                 LIMIT $2 OFFSET $3",
                &[&category, &limit, &offset],
            )
            .await?;
        Ok((total, contentieux, rows.iter().map(directory_row).collect()))
    }

    /// Compteurs de l'annuaire par catégorie (page d'accueil) :
    /// `(category, registre, contentieux)` — matérialisés dans
    /// `annuaire_registre` (rafraîchie au relink).
    pub async fn annuaire_stats(&self) -> Result<Vec<(String, i64, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT category, total, contentieux FROM annuaire_registre",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Itère `(namespace, local_id, lastmod)` pour les entités à publier au
    /// sitemap (pages `/entity/{juris}/{ns}/{id}`). Périmètre = entités avec ≥ 1 décision
    /// liée (`entity.decision_count > 0`) **sauf les avocats**
    /// (`category = 'avocats'`) : ce sont des personnes physiques, exclues de la
    /// promotion SEO pour raison RGPD. `lastmod` = date de la décision liée la plus
    /// récente (`MAX(GREATEST(date_lecture, updated_at::date))`), capée à
    /// `current_date`. `local_id` = uid privé de son préfixe `{namespace}:`.
    ///
    /// The `GROUP BY` over `entity`, `decision_actor` and `decisions` exceeds the
    /// pool's `statement_timeout`: lifted locally in its own transaction.
    #[tracing::instrument(name = "db.iter_entities_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_entities_for_sitemap(
        &self,
    ) -> Result<Vec<(String, String, Option<chrono::NaiveDate>)>> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<(String, String, Option<chrono::NaiveDate>)>> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let rows = self
                .conn
                .query(
                    "
                    SELECT split_part(e.uid, ':', 1) AS namespace,
                           substr(e.uid, strpos(e.uid, ':') + 1) AS local_id,
                           MAX(GREATEST(d.date_lecture, d.updated_at::date))::date
                               AS lastmod
                    FROM entity e
                    JOIN decision_actor p ON p.entity_uid = e.uid
                    JOIN decisions d ON d.id = p.decision_id AND d.deleted_at IS NULL
                    WHERE e.decision_count > 0 AND e.category <> 'avocats'
                    GROUP BY e.uid
                    ORDER BY e.uid
                    ",
                    &[],
                )
                .await?;
            Ok(rows
                .iter()
                .map(|r| (r.get(0), r.get(1), r.get(2)))
                .collect())
        }
        .await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        result
    }
}

/// Mappe une ligne `entity` (colonnes dans l'ordre des SELECT de l'annuaire)
/// vers [`EntityDirectoryRow`].
fn directory_row(r: &tokio_postgres::Row) -> EntityDirectoryRow {
    EntityDirectoryRow {
        uid: r.get(0),
        namespace: r.get(1),
        denomination: r.get(2),
        nature: r.get(3),
        forme: r.get(4),
        active: r.get(5),
        barreau: r.get(6),
        decision_count: r.get(7),
    }
}
