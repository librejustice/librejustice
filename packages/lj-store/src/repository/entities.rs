//! Référentiel d'entités (ADR 0179) : chargement des registres externes
//! (SIRENE, RNA…) par remplacement de namespace + COPY binaire.
//!
//! L'appelant (lj-ingest) streame la source et pousse des lots ; le
//! remplacement est encadré par `entity_namespace_clear` (DELETE du
//! namespace) et les lots par `entity_copy` — le tout dans la
//! transaction que possède l'appelant (comme le pipeline d'ingest).

use super::types::{
    DecisionPartyReadRow, EntityContentieuxCounts, EntityCounselRow, EntityDecisionRow,
    EntityDenominationReadRow, EntityDirectoryRow, EntityHeaderRow, EntityJurisdictionCountRow,
    EntityYearCountRow,
};
use super::DecisionRepository;
use crate::error::Result;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Une entité de registre prête à charger. Aucun folded : le pliage vit
/// dans les index d'expression `lj_fold`/`lj_fold_all` (ADR 0245), jamais
/// en heap.
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
    /// Patronyme composé plié, tirets normalisés en espaces (avocats CNB,
    /// ADR 0195) — clé du sous-étage de résolution nom-seul.
    pub surname_key: Option<String>,
    /// Catégorie d'annuaire (ADR 0239) — dérivée par le chargeur, écrivain
    /// unique de la règle (namespace × nature × APE).
    pub category: &'static str,
    /// Code APE Insee brut (`69.10Z`, `siren:` seulement).
    pub ape: Option<String>,
    /// Slug barreau (`cnb:` seulement, 2ᵉ segment de l'uid).
    pub barreau: Option<String>,
    /// Dénominations supplémentaires du jsonb `denominations` (ADR 0249 :
    /// ordre inverse `NOM PRENOM` et nom commercial des EI) — surface de
    /// résolution GIN, sans date.
    pub alt_denominations: Vec<String>,
}

/// Une dénomination historique datée (période close, hors nom courant) —
/// stagée puis fusionnée dans `entity.denominations` (ADR 0245). Pas de
/// folded : lj_fold_all le calcule à l'indexation.
#[derive(Debug, Clone)]
pub struct EntityHistoryWriteItem {
    pub entity_uid: String,
    pub denomination: String,
    pub date_debut: Option<chrono::NaiveDate>,
    pub date_fin: Option<chrono::NaiveDate>,
}

impl DecisionRepository<'_> {
    /// Vide un namespace (`siren`, `rna`…) avant rechargement complet —
    /// idempotence par remplacement (règle #7). À appeler dans la
    /// transaction du chargeur ; lève le `statement_timeout` du pool pour
    /// le reste de cette transaction (le DELETE de 12,8 M de lignes `siren:`
    /// entretient les index annuaire de l'ADR 0239, au-delà des 30 s).
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
                 sigle, forme, active, surname_key, category, ape, barreau) \
                 FROM STDIN (FORMAT binary)",
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
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
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

    // ── Fiche entité (lecture, ADR 0189) ──────────────────────────────────────

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
    /// `entity.denominations` (ADR 0245) ; tri textuel = chronologique
    /// (dates ISO).
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
                "SELECT count(DISTINCT p.decision_id), \
                        count(DISTINCT p.decision_id) FILTER (WHERE p.side = 'applicant'), \
                        count(DISTINCT p.decision_id) FILTER (WHERE p.side = 'defendant') \
                 FROM decision_party p \
                 JOIN decisions d ON d.id = p.decision_id \
                 WHERE p.entity_uid = $1 AND d.deleted_at IS NULL",
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
                        count(DISTINCT p.decision_id) \
                 FROM decision_party p \
                 JOIN decisions d ON d.id = p.decision_id \
                 WHERE p.entity_uid = $1 AND d.deleted_at IS NULL \
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
                "SELECT d.jurisdiction_code, d.jurisdiction_type, count(DISTINCT p.decision_id) AS n \
                 FROM decision_party p \
                 JOIN decisions d ON d.id = p.decision_id \
                 WHERE p.entity_uid = $1 AND d.deleted_at IS NULL \
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

    /// Top conseils (avocats/cabinets) observés dans les décisions où l'entité
    /// est **partie**, décroissant : co-occurrence `counsel_name`/`law_firm` sur
    /// le même `decision_id`. `entity_uid` = uid registre du conseil s'il est
    /// lui-même résolu.
    pub async fn entity_top_counsel(&self, uid: &str, limit: i64) -> Result<Vec<EntityCounselRow>> {
        let rows = self
            .conn
            .query(
                "WITH anchor AS ( \
                     SELECT DISTINCT decision_id FROM decision_party \
                     WHERE entity_uid = $1 AND quality = 'party' \
                 ) \
                 SELECT p.value, \
                        (array_agg(p.entity_uid) FILTER (WHERE p.entity_uid IS NOT NULL))[1] AS uid, \
                        count(DISTINCT p.decision_id) AS n \
                 FROM decision_party p \
                 JOIN anchor a ON a.decision_id = p.decision_id \
                 WHERE p.quality IN ('counsel_name', 'law_firm') \
                 GROUP BY p.value ORDER BY n DESC, p.value LIMIT $2",
                &[&uid, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| EntityCounselRow {
                value: r.get(0),
                entity_uid: r.get(1),
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
                "SELECT count(DISTINCT p.decision_id) \
                 FROM decision_party p \
                 JOIN decisions d ON d.id = p.decision_id \
                 WHERE p.entity_uid = $1 AND d.deleted_at IS NULL",
                &[&uid],
            )
            .await?
            .get(0);

        let rows = self
            .conn
            .query(
                "SELECT pr.decision_id, pr.quality, pr.side \
                 FROM ( \
                     SELECT DISTINCT ON (decision_id) decision_id, quality, side \
                     FROM decision_party \
                     WHERE entity_uid = $1 \
                     ORDER BY decision_id, \
                       CASE quality WHEN 'party' THEN 0 WHEN 'law_firm' THEN 1 \
                                    WHEN 'counsel_name' THEN 2 ELSE 3 END, ord \
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

    /// Acteurs `decision_party` d'une décision par `public_id`, ordre stable
    /// (`party` > `law_firm` > `counsel_name`, puis `applicant` avant
    /// `defendant`, puis `ord`). `None` = décision inconnue (→ 404) ;
    /// `Some(vec![])` = décision connue sans acteur extrait.
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
                "SELECT p.quality, p.side, p.value, p.nature, p.barreau, p.entity_uid \
                 FROM decision_party p \
                 JOIN decisions d ON d.id = p.decision_id \
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
                })
                .collect(),
        ))
    }

    // ── Annuaire des entités (ADR 0192 / 0239) ─────────────────────────────────

    /// Rafraîchit les compteurs annuaire portés par `entity` (ADR 0239) :
    /// `decision_count` par UPDATE différentiel (pose les nouveaux décomptes de
    /// décisions liées non soft-deleted, remet à 0 les entités déliées), puis
    /// re-seed `annuaire_registre` (totaux registre + contentieux par
    /// catégorie, ADR 0233). Appelé à la fin du relink des parties (règle #7 :
    /// idempotent). Verrou de timeout local comme `resolve_pending_parties`
    /// (l'agrégat balaie `decision_party` entière, au-delà des 30 s du pool).
    /// Renvoie le nombre d'entités avec contentieux.
    #[tracing::instrument(name = "db.refresh_annuaire", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_annuaire(&self) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            self.conn
                .execute(
                    "UPDATE entity e \
                     SET decision_count = cnt.n \
                     FROM ( \
                         SELECT p.entity_uid, count(DISTINCT p.decision_id) AS n \
                         FROM decision_party p \
                         JOIN decisions d ON d.id = p.decision_id \
                         WHERE p.entity_uid IS NOT NULL AND d.deleted_at IS NULL \
                         GROUP BY p.entity_uid \
                     ) cnt \
                     WHERE e.uid = cnt.entity_uid AND e.decision_count <> cnt.n",
                    &[],
                )
                .await?;
            self.conn
                .execute(
                    "UPDATE entity e \
                     SET decision_count = 0 \
                     WHERE e.decision_count > 0 \
                       AND NOT EXISTS ( \
                           SELECT 1 FROM decision_party p \
                           JOIN decisions d ON d.id = p.decision_id \
                           WHERE p.entity_uid = e.uid AND d.deleted_at IS NULL)",
                    &[],
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

    /// Recherche d'entités par préfixe de dénomination pliée (`folded_prefix` déjà
    /// plié côté appelant, du MÊME fold que `lj_fold` — conformité prouvée,
    /// ADR 0245), filtre catégorie
    /// optionnel — sur le registre COMPLET (ADR 0239), en DEUX jambes : le top
    /// contentieux d'abord (index partiel `decision_count > 0`, tri borné à ce
    /// sous-ensemble), puis le complément alphabétique du registre servi par
    /// l'ordre d'`entity_prefix_idx` sans tri (`USING ~<~` :
    /// l'ordre C de l'opclass text_pattern_ops est le seul que cet index sait
    /// servir) — un préfixe court
    /// (« sci  » ≈ 1,9 M de hits) payait ~5 s de top-N sur toute la plage. Le
    /// préfixe est échappé côté appelant (wildcards LIKE neutralisés) et déjà
    /// suffixé `%`.
    pub async fn entity_search(
        &self,
        folded_like: &str,
        category: Option<&str>,
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
                        AND ($2::text IS NULL OR category = $2) \
                        AND decision_count > 0 \
                      ORDER BY decision_count DESC, lj_fold(denomination) \
                      LIMIT $3) \
                     UNION ALL \
                     (SELECT uid, split_part(uid, ':', 1), denomination, \
                             nature, forme, active, barreau, decision_count, \
                             lj_fold(denomination), 1 \
                      FROM entity \
                      WHERE lj_fold(denomination) LIKE $1 ESCAPE '\\' \
                        AND ($2::text IS NULL OR category = $2) \
                        AND decision_count = 0 \
                      ORDER BY lj_fold(denomination) USING ~<~ \
                      LIMIT $3) \
                 ) t \
                 ORDER BY leg, decision_count DESC, df \
                 LIMIT $3",
                &[&folded_like, &category, &limit],
            )
            .await?;
        Ok(rows.iter().map(directory_row).collect())
    }

    /// Listing paginé d'une catégorie de l'annuaire — registre COMPLET, tri
    /// contentieux décroissant puis dénomination (ADR 0239) ; filtre barreau
    /// optionnel (avocats). Renvoie `(total, contentieux, page)` — `total` =
    /// lignes paginables du filtre courant (le registre entier de la
    /// catégorie, ou le sous-ensemble barreau), `contentieux` = celles avec
    /// ≥ 1 décision. Sans barreau les deux viennent d'`annuaire_registre`
    /// (O(1), rafraîchie au relink) ; avec barreau ils se comptent en live
    /// (sous-ensemble petit, index partiel).
    pub async fn entity_directory(
        &self,
        category: &str,
        barreau: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<(i64, i64, Vec<EntityDirectoryRow>)> {
        let counts = self
            .conn
            .query_one(
                "SELECT CASE WHEN $2::text IS NULL THEN \
                            coalesce((SELECT total FROM annuaire_registre \
                                      WHERE category = $1), 0) \
                        ELSE (SELECT count(*) FROM entity \
                              WHERE category = $1 AND barreau = $2) END, \
                        CASE WHEN $2::text IS NULL THEN \
                            coalesce((SELECT contentieux FROM annuaire_registre \
                                      WHERE category = $1), 0) \
                        ELSE (SELECT count(*) FROM entity \
                              WHERE category = $1 AND barreau = $2 \
                                AND decision_count > 0) END",
                &[&category, &barreau],
            )
            .await?;
        let (total, contentieux): (i64, i64) = (counts.get(0), counts.get(1));
        let rows = self
            .conn
            .query(
                "SELECT uid, split_part(uid, ':', 1), denomination, nature, forme, \
                        active, barreau, decision_count \
                 FROM entity \
                 WHERE category = $1 AND ($2::text IS NULL OR barreau = $2) \
                 ORDER BY decision_count DESC, lj_fold(denomination) \
                 LIMIT $3 OFFSET $4",
                &[&category, &barreau, &limit, &offset],
            )
            .await?;
        Ok((total, contentieux, rows.iter().map(directory_row).collect()))
    }

    /// Compteurs de l'annuaire par catégorie (page d'accueil) :
    /// `(category, registre, contentieux)` — matérialisés dans
    /// `annuaire_registre` (rafraîchie au relink, ADR 0233/0239).
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
    /// sitemap (pages `/entite/{ns}/{id}`, ADR 0237). Périmètre = entités
    /// avec ≥ 1 décision liée (`entity.decision_count > 0`, ADR 0239) **sauf
    /// les avocats** (`category = 'avocats'`) : ce sont des personnes
    /// physiques, exclues de la promotion SEO pour raison RGPD. `lastmod` =
    /// date de la décision liée la plus récente
    /// (`MAX(GREATEST(date_lecture, updated_at::date))`), capée à
    /// `current_date`. `local_id` = uid privé de son préfixe `{namespace}:`.
    ///
    /// Le `GROUP BY` sur le join `entity × decision_party × decisions`
    /// dépasse le `statement_timeout` du pool (30 s) sur le corpus de prod :
    /// levé localement dans une transaction dédiée (même pattern que
    /// `refresh_article_code_titles`). Batch quotidien du cron, pas de chemin
    /// requête utilisateur.
    #[tracing::instrument(name = "db.iter_entities_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_entities_for_sitemap(
        &self,
    ) -> Result<Vec<(String, String, chrono::NaiveDate)>> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<(String, String, chrono::NaiveDate)>> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let rows = self
                .conn
                .query(
                    "
                    SELECT split_part(e.uid, ':', 1) AS namespace,
                           substr(e.uid, strpos(e.uid, ':') + 1) AS local_id,
                           LEAST(
                               MAX(GREATEST(d.date_lecture, d.updated_at::date)),
                               current_date
                           )::date AS lastmod
                    FROM entity e
                    JOIN decision_party p ON p.entity_uid = e.uid
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
