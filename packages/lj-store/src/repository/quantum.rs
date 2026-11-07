//! Le quantum du préjudice corporel : le record entier en `jsonb` sur
//! `decision_quantum`, sa projection en observations sur `quantum_observation`.
//!
//! One reading per decision. `pc_extract_version` tags the row: the ground truth
//! carries 1000, an automatic pass `PC_EXTRACT_VERSION`, and a pass never
//! replaces a row tagged higher than itself.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde_json::Value;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

use super::DecisionRepository;
use crate::error::Result;

/// One row of the distribution. Amounts are in integer hundredths: a median and
/// a sum over the whole table must be exact, and folding from the euros of the
/// read schema belongs to the caller.
#[derive(Debug, Clone, Default)]
pub struct QuantumObservationRow {
    pub victime_ord: i16,
    pub poste_ord: i16,
    /// -1 on the poste's own row, the only one carrying `montant_cents`: a
    /// finer row observes a price alone, so summing the amounts still sums the
    /// postes.
    pub composante_ord: i16,
    pub tranche_ord: i16,
    pub poste: String,
    pub axe: Option<String>,
    pub hors_table: Option<String>,
    pub montant_cents: Option<i64>,
    pub valeur_unitaire_cents: Option<i64>,
    pub quantite: Option<f64>,
    pub unite: Option<String>,
    pub nature_aide: Option<String>,
    pub mode_emploi: Option<String>,
    pub specialisee: Option<bool>,
    pub nuit: Option<bool>,
    pub taux_pct: Option<f64>,
    pub cotation: Option<f64>,
    pub categorie: Option<String>,
    pub categorie_manque: Option<String>,
    pub deces: Option<bool>,
    pub comparabilite: Option<Value>,
    pub taux_dfp_pct: Option<f64>,
    pub age: Option<i16>,
    pub sexe: Option<String>,
    pub role: Option<String>,
    pub lien: Option<String>,
    pub preuve: Option<String>,
    pub equations: i16,
    pub origine: Option<String>,
    pub cite: Option<String>,
    pub indemnites_acquises_cents: Option<i64>,
}

/// Un record à écrire, avec ce que la validation en a dit.
#[derive(Debug, Clone)]
pub struct QuantumWriteItem {
    pub decision_id: i64,
    pub est_liquidation: bool,
    pub atteste: Option<bool>,
    pub raison_non_atteste: Option<String>,
    pub cases_confirmees: Option<i32>,
    pub quantum: Value,
    pub observations: Vec<QuantumObservationRow>,
}

const COPY_SQL: &str = "COPY quantum_observation (decision_id, pc_extract_version, victime_ord, \
     poste_ord, composante_ord, tranche_ord, poste, axe, hors_table, montant_cents, \
     valeur_unitaire_cents, quantite, unite, nature_aide, mode_emploi, specialisee, nuit, \
     taux_pct, cotation, categorie, categorie_manque, deces, comparabilite, taux_dfp_pct, \
     age, sexe, role, lien, preuve, equations, origine, cite, indemnites_acquises_cents, \
     juridiction, annee) \
     FROM STDIN (FORMAT binary)";
const COPY_TYPES: &[Type] = &[
    Type::INT8,
    Type::INT2,
    Type::INT2,
    Type::INT2,
    Type::INT2,
    Type::INT2,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::INT8,
    Type::INT8,
    Type::FLOAT8,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::BOOL,
    Type::BOOL,
    Type::FLOAT8,
    Type::FLOAT8,
    Type::TEXT,
    Type::TEXT,
    Type::BOOL,
    Type::JSONB,
    Type::FLOAT8,
    Type::INT2,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::INT2,
    Type::TEXT,
    Type::TEXT,
    Type::INT8,
    Type::TEXT,
    Type::INT2,
];

impl DecisionRepository<'_> {
    /// The decisions naming a poste of the corporel, by id: the fonds the
    /// deterministic pass reads.
    #[tracing::instrument(name = "db.quantum_fonds_ids", skip(self), fields(db.system = "postgresql"))]
    pub async fn quantum_fonds_ids(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "SELECT d.decision_id FROM decision_text d
                 WHERE d.deleted_at IS NULL AND d.lang = 'fr'
                   AND d.id @@@ paradedb.boolean(should => ARRAY[
                     paradedb.phrase('full_text', ARRAY['déficit','fonctionnel','permanent']),
                     paradedb.phrase('full_text', ARRAY['déficit','fonctionnel','temporaire']),
                     paradedb.phrase('full_text', ARRAY['souffrances','endurées']),
                     paradedb.phrase('full_text', ARRAY['tierce','personne']),
                     paradedb.phrase('full_text', ARRAY['préjudice','d','affection'])])
                 ORDER BY d.decision_id",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get::<_, i64>(0)).collect())
    }

    /// `(id, full_text)` of the given decisions, the deleted ones left out.
    #[tracing::instrument(name = "db.decision_texts", skip(self, ids), fields(db.system = "postgresql", ids = ids.len()))]
    pub async fn decision_texts(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT decision_id, full_text FROM decision_text
                 WHERE decision_id = ANY($1) AND lang = 'fr' AND deleted_at IS NULL
                 ORDER BY decision_id",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }

    /// Drops the readings of these decisions tagged at most `pc_extract_version`,
    /// observations with them: what a pass wrote and no longer finds. A manual
    /// revision stays.
    #[tracing::instrument(name = "db.delete_quantum_at_most", skip(self, ids), fields(db.system = "postgresql", ids = ids.len()))]
    pub async fn delete_quantum_at_most(
        &self,
        ids: &[i64],
        pc_extract_version: i16,
    ) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "DELETE FROM decision_quantum
                 WHERE decision_id = ANY($1) AND pc_extract_version <= $2",
                &[&ids, &pc_extract_version],
            )
            .await?)
    }

    /// Writes a batch of records tagged with a version, and replaces their
    /// observations.
    ///
    /// **The transaction belongs to the caller.** One reading per decision: a
    /// row tagged with a higher version, a manual revision, is left untouched,
    /// observations included. Returns the number of observations written.
    #[tracing::instrument(name = "db.replace_quantum_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_quantum_bulk(
        &self,
        items: &[QuantumWriteItem],
        pc_extract_version: i16,
    ) -> Result<usize> {
        let mut written: Vec<&QuantumWriteItem> = Vec::with_capacity(items.len());
        for it in items {
            let row = self
                .conn
                .query_opt(
                    "
                    INSERT INTO decision_quantum (decision_id, pc_extract_version, est_liquidation,
                                                  atteste, raison_non_atteste, cases_confirmees,
                                                  quantum, updated_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, now())
                    ON CONFLICT (decision_id) DO UPDATE SET
                      pc_extract_version = EXCLUDED.pc_extract_version,
                      est_liquidation    = EXCLUDED.est_liquidation,
                      atteste            = EXCLUDED.atteste,
                      raison_non_atteste = EXCLUDED.raison_non_atteste,
                      cases_confirmees   = EXCLUDED.cases_confirmees,
                      quantum            = EXCLUDED.quantum,
                      updated_at         = now()
                    WHERE decision_quantum.pc_extract_version <= EXCLUDED.pc_extract_version
                    RETURNING decision_id
                    ",
                    &[
                        &it.decision_id,
                        &pc_extract_version,
                        &it.est_liquidation,
                        &it.atteste,
                        &it.raison_non_atteste,
                        &it.cases_confirmees,
                        &it.quantum,
                    ],
                )
                .await?;
            if row.is_some() {
                written.push(it);
            }
        }
        if written.is_empty() {
            return Ok(0);
        }
        let ids: Vec<i64> = written.iter().map(|i| i.decision_id).collect();

        self.conn
            .execute(
                "DELETE FROM quantum_observation WHERE decision_id = ANY($1)",
                &[&ids],
            )
            .await?;

        let contexte: HashMap<i64, (Option<String>, Option<i16>)> = self
            .conn
            .query(
                "SELECT id, jurisdiction_code, date_lecture FROM decisions WHERE id = ANY($1)",
                &[&ids],
            )
            .await?
            .into_iter()
            .map(|r| {
                let annee = r
                    .get::<_, Option<NaiveDate>>(2)
                    .and_then(|d| d.format("%Y").to_string().parse::<i16>().ok());
                (r.get::<_, i64>(0), (r.get(1), annee))
            })
            .collect();

        let sink = self.conn.copy_in(COPY_SQL).await?;
        let writer = BinaryCopyInWriter::new(sink, COPY_TYPES);
        tokio::pin!(writer);
        let mut ecrites = 0usize;
        for it in written {
            let (juridiction, annee) = contexte.get(&it.decision_id).cloned().unwrap_or_default();
            for o in &it.observations {
                writer
                    .as_mut()
                    .write(&[
                        &it.decision_id,
                        &pc_extract_version,
                        &o.victime_ord,
                        &o.poste_ord,
                        &o.composante_ord,
                        &o.tranche_ord,
                        &o.poste,
                        &o.axe,
                        &o.hors_table,
                        &o.montant_cents,
                        &o.valeur_unitaire_cents,
                        &o.quantite,
                        &o.unite,
                        &o.nature_aide,
                        &o.mode_emploi,
                        &o.specialisee,
                        &o.nuit,
                        &o.taux_pct,
                        &o.cotation,
                        &o.categorie,
                        &o.categorie_manque,
                        &o.deces,
                        &o.comparabilite,
                        &o.taux_dfp_pct,
                        &o.age,
                        &o.sexe,
                        &o.role,
                        &o.lien,
                        &o.preuve,
                        &o.equations,
                        &o.origine,
                        &o.cite,
                        &o.indemnites_acquises_cents,
                        &juridiction,
                        &annee,
                    ])
                    .await?;
                ecrites += 1;
            }
        }
        writer.finish().await?;
        Ok(ecrites)
    }

    /// Résout des `source_uid` vers leurs `decision_id`. Les uids inconnus sont
    /// absents du résultat, jamais une erreur : la vérité terrain nomme des
    /// décisions que le fonds n'a pas forcément encore.
    #[tracing::instrument(name = "db.resolve_source_uids", skip_all, fields(db.system = "postgresql"))]
    pub async fn resolve_source_uids(&self, source_uids: &[String]) -> Result<Vec<(String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT ds.source_uid, ds.decision_id \
                 FROM decision_sources ds JOIN decisions d ON d.id = ds.decision_id \
                 WHERE ds.source_uid = ANY($1) \
                   AND ds.deleted_at IS NULL AND d.deleted_at IS NULL",
                &[&source_uids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
            .collect())
    }
}
