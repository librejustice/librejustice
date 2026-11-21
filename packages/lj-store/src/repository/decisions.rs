//! Upsert des métadonnées décision et écriture du contenu canonique :
//! `upsert`/`update_existing`, application de provenance, écriture canonique,
//! création, et mise à jour batchée des champs ré-extractibles.

use super::decision_text::original_lang;
use super::support::{
    as_param_refs, extracted_column_type, extracted_field_value, formation_paths, is_routed_field,
    now, CASE_CITATIONS_FIELD, CASE_NAME_FIELD, DECISION_LINKS_FIELD, LEGAL_REFS_FIELD,
    PARTIES_FIELD, REEXTRACTABLE_FIELDS,
};
use super::types::{ExtractedFields, Identity, JurisdictionRow, UpsertResult, UpsertStatus};
use super::{DecisionRepository, ElectedCaseName};
use crate::error::{Result, StoreError};
use lj_core::decision::Decision;
use lj_versions::EXTRACT_VERSION;
use serde_json::Value;
use tokio_postgres::types::{Json, ToSql};

impl DecisionRepository<'_> {
    #[tracing::instrument(name = "db.count_decisions", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_decisions(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one("SELECT COUNT(*) FROM decisions", &[])
            .await?;
        Ok(row.get::<_, i64>(0))
    }

    /// Identité de cible d'un lot de décisions actives — l'entrée de la sonde
    /// inversée (`attract_pending_citations`), lue aux valeurs STOCKÉES :
    /// `(jurisdiction_code, docket_numbers)`.
    pub async fn decision_target_identities(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(Option<String>, Vec<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT jurisdiction_code, docket_numbers \
                 FROM decisions WHERE id = ANY($1) AND deleted_at IS NULL",
                &[&ids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    #[tracing::instrument(name = "db.upsert", skip(self, decision, extracted), fields(db.system = "postgresql"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        decision: &Decision,
        content_checksum: &str,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        source_fields: &Value,
        payload_format: &str,
        distinct: bool,
    ) -> Result<UpsertResult> {
        if decision.jurisdiction_type.is_none() {
            return Err(StoreError::Invalid(format!(
                "Décision {} : jurisdiction_type inconnue, refus d'insertion (contrainte NOT NULL).",
                decision.source_uid
            )));
        }

        // L'upsert est **piloté par `source_uid`** (pivot d'idempotence sur
        // `decision_sources`).
        if let Some((id, stored_checksum, active)) =
            self.find_provenance(&decision.source_uid).await?
        {
            // Provenance connue (active OU tombstonée) → ré-update en place, même
            // décision. Skip seulement si active ET checksum inchangé ; une provenance
            // tombstonée se ressuscite même à checksum égal.
            if (active && stored_checksum == content_checksum)
                || self
                    .is_expired(&decision.source_uid, content_checksum)
                    .await?
            {
                return Ok(UpsertResult {
                    id,
                    status: UpsertStatus::Skipped,
                });
            }
            self.apply_provenance(
                id,
                decision,
                content_checksum,
                public_id,
                extracted,
                canonical_ref,
                source_fields,
                payload_format,
            )
            .await?;
            return Ok(UpsertResult {
                id,
                status: UpsertStatus::Updated,
            });
        }

        let identity = if distinct {
            Identity::Create
        } else {
            self.identity_of(decision, extracted, source_fields).await?
        };
        match identity {
            Identity::Attach(existing_id) => {
                self.apply_provenance(
                    existing_id,
                    decision,
                    content_checksum,
                    public_id,
                    extracted,
                    canonical_ref,
                    source_fields,
                    payload_format,
                )
                .await?;
                return Ok(UpsertResult {
                    id: existing_id,
                    status: UpsertStatus::Updated,
                });
            }
            Identity::Copy(id) => {
                return Ok(UpsertResult {
                    id,
                    status: UpsertStatus::Copy,
                })
            }
            Identity::Hold(id) => {
                return Ok(UpsertResult {
                    id,
                    status: UpsertStatus::Held,
                })
            }
            Identity::Create => {}
        }

        let id = self
            .insert_decision(
                decision,
                content_checksum,
                public_id,
                extracted,
                canonical_ref,
                source_fields,
                payload_format,
            )
            .await?;
        Ok(UpsertResult {
            id,
            status: UpsertStatus::Created,
        })
    }

    /// Applique la provenance entrante à une décision EXISTANTE `decision_id`,
    /// sous verrou xact sur la décision. (1) upsert de la `decision_sources` (porte
    /// `source_fields`, lève le tombstone) ; (2) réécrit le contenu canonique
    /// (`full_text`/métadonnées) **SSI l'entrant est (ou devient) l'autorité** —
    /// provenance de rang max : une source de rang inférieur attache ses méta sans
    /// toucher au texte servi (garde RGPD) ; (3) `reconcile` (état `deleted_at` /
    /// vide).
    #[allow(clippy::too_many_arguments)]
    async fn apply_provenance(
        &self,
        decision_id: i64,
        decision: &Decision,
        content_checksum: &str,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        source_fields: &Value,
        payload_format: &str,
    ) -> Result<()> {
        // Verrou par décision (relâché en fin de transaction de l'appelant) :
        // sérialise upsert et retrait concurrents sur la même décision.
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&decision_id])
            .await?;
        // 1) Provenance : porte `source_fields`, (ré)active (deleted_at = NULL).
        self.upsert_decision_source(
            decision_id,
            &decision.source_uid,
            content_checksum,
            payload_format,
            source_fields,
        )
        .await?;
        // 2) Contenu canonique : seule l'autorité (rang max) écrit
        //    `full_text`/métadonnées. La provenance vient d'être upsertée, donc elle
        //    entre dans le calcul de l'autorité.
        if self.authority_source_uid(decision_id).await?.as_deref()
            == Some(decision.source_uid.as_str())
        {
            self.write_canonical_content(
                decision_id,
                decision,
                public_id,
                extracted,
                canonical_ref,
                source_fields,
            )
            .await?;
            if let Some(e) = extracted {
                self.set_source_case_names(&[(decision_id, e.case_name.as_deref())])
                    .await?;
                self.replace_citations(decision_id, e.citation_occurrences.as_deref())
                    .await?;
                self.replace_decision_links(decision_id, e.decision_links.as_deref())
                    .await?;
                self.replace_case_citations(decision_id, e.case_citations.as_deref())
                    .await?;
                self.replace_decision_actors(decision_id, e.parties.as_deref())
                    .await?;
            }
        } else {
            self.put_text(
                decision_id,
                &decision.source_uid,
                source_fields,
                &decision.texte_integral_clean,
            )
            .await?;
        }
        // 3) Reconcile : `deleted_at` / vide RGPD (no-op si autorité active).
        self.reconcile(decision_id).await?;
        Ok(())
    }

    /// Réécrit les colonnes **canoniques** d'une décision depuis sa provenance
    /// autoritaire : métadonnées coalescées, `full_text` (texte de l'autorité),
    /// identité (`ecli`/`canonical_ref`), versions. N'écrit **rien** de spécifique à
    /// une provenance (`source_uid`/`content_checksum`/`source_fields` vivent sur
    /// `decision_sources`). No-op sur une décision à `extract_version` >
    /// `EXTRACT_VERSION` : un écrivain à version V ne remplace jamais des données à
    /// version > V.
    #[allow(clippy::too_many_arguments)]
    async fn write_canonical_content(
        &self,
        decision_id: i64,
        decision: &Decision,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        source_fields: &Value,
    ) -> Result<()> {
        let e = extracted;
        let jurisdiction_code = match e.and_then(|e| e.jurisdiction.clone()) {
            Some(j) => self
                .ensure_jurisdictions(std::slice::from_ref(&j))
                .await?
                .remove(&j.source_code),
            None => None,
        };
        let written = self
            .conn
            .execute(
                "
                UPDATE decisions SET
                  jurisdiction_type = $1,
                  public_id = coalesce(public_id, $2), updated_at = $3,
                  date_lecture = $4, date_audience = $5, docket_numbers = $6,
                  publication_codes = $7,
                  extract_version = $9,
                  ecli = $10, canonical_ref = $11,
                  solution_uid = $12, procedure_uid = $13, office_uid = $14,
                  legal_domain_uid = $15,
                  jurisdiction_code = $16,
                  themes = $17,
                  formation_uid = $18,
                  search_title = $19,
                  seat = $20, seat_paths = $21, formation_paths = $22,
                  kind = $25,
                  lang = CASE WHEN EXISTS (SELECT 1 FROM decision_sources s
                                            WHERE s.decision_id = $8 AND s.source_uid <> $24
                                              AND s.deleted_at IS NULL)
                              THEN lang ELSE $23 END
                WHERE id = $8
                  AND (extract_version IS NULL OR extract_version <= $9)
                ",
                &[
                    &decision.jurisdiction_type,
                    &public_id,
                    &now(),
                    &e.and_then(|e| e.date_lecture),
                    &e.and_then(|e| e.date_audience),
                    &e.map(|e| e.docket_numbers.clone()).unwrap_or_default(),
                    &e.map(|e| e.publication_codes.clone()).unwrap_or_default(),
                    &decision_id,
                    &EXTRACT_VERSION,
                    &decision.ecli,
                    &canonical_ref,
                    &e.and_then(|e| e.solution_uid.clone()),
                    &e.and_then(|e| e.procedure_uid.clone()),
                    &e.and_then(|e| e.office_uid.clone()),
                    &e.and_then(|e| e.legal_domain_uid.clone()),
                    &jurisdiction_code,
                    &e.map(|e| e.themes.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.formation_uid.clone()),
                    &e.and_then(|e| e.search_title.clone()),
                    &e.and_then(|e| e.seat.clone()).map(Json),
                    &e.map(|e| e.seat_paths.clone()).unwrap_or_default(),
                    &e.map(formation_paths).unwrap_or_default(),
                    &original_lang(source_fields)?,
                    &decision.source_uid,
                    &e.map(|e| e.kind).unwrap_or_default().as_str(),
                ],
            )
            .await?;
        if written == 1 {
            self.put_text(
                decision_id,
                &decision.source_uid,
                source_fields,
                &decision.texte_integral_clean,
            )
            .await?;
        }
        Ok(())
    }

    /// Le nom que la source de la décision porte elle-même, posé par
    /// [`Self::put_case_names`]. Une décision que la source ne nomme plus perd
    /// le sien : la ré-extraction la laisserait autrement sur un nom que sa
    /// source ne donne plus.
    async fn set_source_case_names(&self, items: &[(i64, Option<&str>)]) -> Result<u64> {
        let elected: Vec<ElectedCaseName> = items
            .iter()
            .filter_map(|(decision_id, name)| {
                let name = (*name)?;
                Some(ElectedCaseName {
                    decision_id: *decision_id,
                    keys: vec![lj_core::case_names::case_name_key(name)],
                    name: Some(name.to_string()),
                    source: "source",
                })
            })
            .collect();
        let orphelines: Vec<i64> = items
            .iter()
            .filter(|(_, name)| name.is_none())
            .map(|(decision_id, _)| *decision_id)
            .collect();
        let efface = self.clear_case_names("source", &orphelines).await?;
        Ok(efface.len() as u64 + self.put_case_names(&elected).await?)
    }

    /// Crée les lignes du référentiel `jurisdiction` manquantes — référentiel
    /// OUVERT nourri par la donnée. L'entrée est keyée par `source_code` (code de la
    /// source, ex. location Judilibre `tj75056`) ; le code canonique (`tj_paris`)
    /// est calculé à la création. Renvoie la map `source_code` → code canonique
    /// pour `decisions.jurisdiction_code`. Conflit sur `source_code` : la ligne
    /// existante ne bouge que si la nouvelle est plus disante (elle apporte la ville
    /// et l'existante n'en a pas) — un libellé source nu (« Tribunal judiciaire »)
    /// ne dégrade jamais un label complet, et une ligne née nue guérit dès qu'une
    /// décision du même code porte la ville (son `code`, lui, reste celui de la
    /// création).
    pub async fn ensure_jurisdictions(
        &self,
        rows: &[JurisdictionRow],
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut resolved = std::collections::HashMap::new();
        if rows.is_empty() {
            return Ok(resolved);
        }
        // Dédup par code en ordre TRIÉ : l'upsert verrouille ses lignes dans l'ordre
        // du VALUES et les tient jusqu'au COMMIT du lot appelant. Un ordre non
        // déterministe entre workers concurrents deadlocke.
        let mut by_code: std::collections::BTreeMap<&str, &JurisdictionRow> =
            std::collections::BTreeMap::new();
        for r in rows {
            by_code.entry(r.source_code.as_str()).or_insert(r);
        }
        // Pré-filtre lecture seule : `ON CONFLICT DO UPDATE` verrouille CHAQUE ligne
        // conflictuelle avant d'évaluer son WHERE, et les workers se sérialiseraient
        // sur les codes chauds. On n'upserte que les codes absents ou guérissables ; la
        // course résiduelle (deux workers découvrent le même code neuf) est absorbée
        // par l'ON CONFLICT.
        let all_sources: Vec<&str> = by_code.keys().copied().collect();
        let existing: std::collections::HashMap<String, (String, bool)> = self
            .conn
            .query(
                "SELECT source_code, code, city IS NOT NULL FROM jurisdiction \
                 WHERE source_code = ANY($1)",
                &[&all_sources],
            )
            .await?
            .into_iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2))))
            .collect();
        // Codes canoniques des sources nouvelles : un code déjà porté par une AUTRE
        // ligne est une variante de nom de la même cour (« TA de St Barthélemy » émis
        // quand la ligne canonique dit « Saint-Barthélemy ») — on résout vers cette
        // ligne, le PK `code` interdit une seconde.
        let candidates: Vec<String> = by_code
            .iter()
            .filter(|(source, _)| !existing.contains_key(**source))
            .map(|(source, r)| {
                canonical_jurisdiction_code(&r.jurisdiction_type, r.city.as_deref(), source)
            })
            .collect();
        let mut taken: std::collections::HashSet<String> = if candidates.is_empty() {
            Default::default()
        } else {
            self.conn
                .query(
                    "SELECT code FROM jurisdiction WHERE code = ANY($1)",
                    &[&candidates],
                )
                .await?
                .into_iter()
                .map(|r| r.get(0))
                .collect()
        };
        let (mut codes, mut sources, mut types, mut cities, mut labels, mut places) = (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        for (source, r) in by_code {
            let code = match existing.get(source) {
                Some((code, complete)) => {
                    resolved.insert(source.to_string(), code.clone());
                    if *complete || r.city.is_none() {
                        continue; // complète / rien à guérir
                    }
                    code.clone()
                }
                None => {
                    let code = canonical_jurisdiction_code(
                        &r.jurisdiction_type,
                        r.city.as_deref(),
                        source,
                    );
                    resolved.insert(source.to_string(), code.clone());
                    if !taken.insert(code.clone()) {
                        continue; // code porté par une autre ligne (ou doublon du lot)
                    }
                    code
                }
            };
            codes.push(code);
            sources.push(source.to_string());
            types.push(r.jurisdiction_type.clone());
            cities.push(r.city.clone());
            labels.push(r.label.clone());
            places.push(r.place.clone());
        }
        if codes.is_empty() {
            return Ok(resolved);
        }
        self.conn
            .execute(
                "INSERT INTO jurisdiction (code, source_code, jurisdiction_type, city, label, place)
                 SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[])
                 ON CONFLICT (source_code) DO UPDATE
                     SET city = EXCLUDED.city, label = EXCLUDED.label
                 WHERE jurisdiction.city IS NULL AND EXCLUDED.city IS NOT NULL",
                &[&codes, &sources, &types, &cities, &labels, &places],
            )
            .await?;
        Ok(resolved)
    }

    /// The labels these courts carry, by code.
    #[tracing::instrument(name = "db.jurisdiction_labels", skip_all, fields(db.system = "postgresql"))]
    pub async fn jurisdiction_labels(
        &self,
        codes: &[String],
    ) -> Result<std::collections::HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT code, label FROM jurisdiction WHERE code = ANY($1)",
                &[&codes],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Renames courts by code, for a fonds that alone names them.
    #[tracing::instrument(name = "db.relabel_jurisdictions", skip_all, fields(db.system = "postgresql"))]
    pub async fn relabel_jurisdictions(&self, codes: &[String], labels: &[String]) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE jurisdiction j SET label = u.label \
                 FROM UNNEST($1::text[], $2::text[]) AS u(code, label) WHERE j.code = u.code",
                &[&codes, &labels],
            )
            .await?)
    }

    /// `jurisdiction_type` and `jurisdiction_code` of a live decision, by its public id.
    #[tracing::instrument(name = "db.decision_jurisdiction", skip(self), fields(db.system = "postgresql"))]
    pub async fn decision_jurisdiction(
        &self,
        public_id: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT jurisdiction_type, jurisdiction_code FROM decisions \
                 WHERE public_id = $1 AND deleted_at IS NULL",
                &[&public_id],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    /// Held until the session ends: one ingest chain at a time.
    pub async fn chain_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one("SELECT pg_try_advisory_lock(hashtext('ingest_chain'))", &[])
            .await?
            .get(0))
    }

    /// Held until the session ends: one JuriCAF sync at a time fetches and loads.
    pub async fn juricaf_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one("SELECT pg_try_advisory_lock(hashtext('juricaf_sync'))", &[])
            .await?
            .get(0))
    }

    /// Held until the session ends: one juportal sync at a time fetches and loads.
    pub async fn juportal_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('juportal_sync'))",
                &[],
            )
            .await?
            .get(0))
    }

    /// Held until the session ends: one Rechtspraak sync at a time fetches and loads.
    pub async fn rechtspraak_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('rechtspraak_sync'))",
                &[],
            )
            .await?
            .get(0))
    }

    /// Held until the session ends: one data.public.lu sync at a time fetches and loads.
    pub async fn justice_lu_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('justice_lu_sync'))",
                &[],
            )
            .await?
            .get(0))
    }

    /// Held until the session ends: one entscheidsuche sync at a time fetches and loads each set
    /// of courts, the empty set naming them all.
    pub async fn entscheidsuche_lock(&self, courts: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('entscheidsuche_sync:' || $1))",
                &[&courts],
            )
            .await?
            .get(0))
    }

    /// Every entscheidsuche provenance by its index id: the day its document was scraped, and
    /// whether its text was read from the original file rather than the index.
    #[tracing::instrument(name = "db.entscheidsuche_held", skip(self), fields(db.system = "postgresql"))]
    pub async fn entscheidsuche_held(&self) -> Result<Vec<(String, Option<String>, bool)>> {
        let rows = self
            .conn
            .query(
                "SELECT substr(source_uid, length('entscheidsuche/') + 1), \
                        source_fields->>'scrapedate', \
                        coalesce((source_fields->>'text_from_original')::boolean, false) \
                 FROM decision_sources WHERE source = 'entscheidsuche'",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// The decision of each of these provenances the base holds, and whether it has a summary.
    #[tracing::instrument(name = "db.provenance_decisions", skip_all, fields(db.system = "postgresql", n = uids.len()))]
    pub async fn provenance_decisions(
        &self,
        uids: &[String],
    ) -> Result<std::collections::HashMap<String, (i64, bool)>> {
        let rows = self
            .conn
            .query(
                "SELECT s.source_uid, s.decision_id, \
                        EXISTS (SELECT 1 FROM decision_text t \
                                WHERE t.decision_id = s.decision_id AND t.summary IS NOT NULL) \
                 FROM decision_sources s WHERE s.source_uid = ANY($1)",
                &[&uids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2))))
            .collect())
    }

    /// The md5 of the text each of these live decisions serves.
    #[tracing::instrument(name = "db.text_digests", skip_all, fields(db.system = "postgresql", n = ids.len()))]
    pub async fn text_digests(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT ON (t.decision_id) t.decision_id, md5(t.full_text) \
                 FROM decision_text t JOIN decisions d ON d.id = t.decision_id \
                 WHERE t.decision_id = ANY($1) AND d.deleted_at IS NULL \
                 ORDER BY t.decision_id, t.id",
                &[&ids],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The day and the text each of these live decisions serves.
    #[tracing::instrument(name = "db.days_and_texts", skip_all, fields(db.system = "postgresql", n = ids.len()))]
    pub async fn days_and_texts(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, (Option<String>, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT ON (t.decision_id) t.decision_id, d.date_lecture::text, t.full_text \
                 FROM decision_text t JOIN decisions d ON d.id = t.decision_id \
                 WHERE t.decision_id = ANY($1) AND d.deleted_at IS NULL \
                 ORDER BY t.decision_id, t.id",
                &[&ids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2))))
            .collect())
    }

    /// Gives a provenance, and the text it wrote, the `source_uid` its source now names it by.
    #[tracing::instrument(name = "db.rename_provenance", skip(self), fields(db.system = "postgresql"))]
    pub async fn rename_provenance(&self, from: &str, to: &str) -> Result<()> {
        let renamed: i64 = self
            .conn
            .query_one(
                "WITH s AS (UPDATE decision_sources SET source_uid = $2 WHERE source_uid = $1 \
                            RETURNING decision_id), \
                      t AS (UPDATE decision_text t SET source_uid = $2 FROM s \
                            WHERE t.decision_id = s.decision_id AND t.source_uid = $1) \
                 SELECT count(*) FROM s",
                &[&from, &to],
            )
            .await?
            .get(0);
        if renamed != 1 {
            return Err(StoreError::Invalid(format!(
                "rename_provenance {from} -> {to}: {renamed} rows"
            )));
        }
        Ok(())
    }

    /// Held until the session ends: one sync of a court's own site at a time fetches and loads.
    pub async fn court_site_lock(&self, source: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('court_site_sync:' || $1))",
                &[&source],
            )
            .await?
            .get(0))
    }

    /// Held until the session ends: one Légimonaco sync at a time fetches and loads.
    pub async fn legimonaco_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('legimonaco_sync'))",
                &[],
            )
            .await?
            .get(0))
    }

    /// Held until the session ends: one Legalis sync at a time fetches and loads.
    pub async fn legalis_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one("SELECT pg_try_advisory_lock(hashtext('legalis_sync'))", &[])
            .await?
            .get(0))
    }

    /// Held until the session ends: one Défenseur des droits sync at a time reads its catalogue.
    pub async fn ddd_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one("SELECT pg_try_advisory_lock(hashtext('ddd_sync'))", &[])
            .await?
            .get(0))
    }

    /// Held until the session ends: one Supreme Court of Canada sync at a time fetches and loads.
    pub async fn scc_csc_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one("SELECT pg_try_advisory_lock(hashtext('scc_csc_sync'))", &[])
            .await?
            .get(0))
    }

    /// Held until the session ends: one Conseil du contentieux des étrangers sync at a time.
    pub async fn cce_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one("SELECT pg_try_advisory_lock(hashtext('cce_sync'))", &[])
            .await?
            .get(0))
    }

    /// `(source_uid, public_id)` of the active decisions these provenances sit on.
    #[tracing::instrument(name = "db.decisions_by_source_uid", skip(self), fields(db.system = "postgresql"))]
    pub async fn decisions_by_source_uid(
        &self,
        source_uids: &[String],
    ) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT s.source_uid, d.public_id \
                 FROM decision_sources s JOIN decisions d ON d.id = s.decision_id \
                 WHERE s.source_uid = ANY($1) AND s.deleted_at IS NULL \
                   AND d.deleted_at IS NULL \
                 ORDER BY s.source_uid",
                &[&source_uids],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    #[tracing::instrument(name = "db.set_public_id", skip(self), fields(db.system = "postgresql"))]
    pub async fn set_public_id(&self, decision_id: i64, public_id: &str) -> Result<()> {
        self.conn
            .execute(
                "
                UPDATE decisions
                SET public_id = $1, updated_at = $2
                WHERE id = $3 AND public_id IS NULL
                ",
                &[&public_id, &now(), &decision_id],
            )
            .await?;
        Ok(())
    }

    /// Met à jour une décision dont l'id est **déjà résolu** (chemin pipeline
    /// `decision_id` connu : re-embed, backfill). Applique la provenance entrante
    /// via [`apply_provenance`](Self::apply_provenance), sans re-sonder l'identité.
    #[tracing::instrument(name = "db.update_existing", skip(self, decision, extracted), fields(db.system = "postgresql"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn update_existing(
        &self,
        decision_id: i64,
        decision: &Decision,
        content_checksum: &str,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        source_fields: &Value,
        payload_format: &str,
    ) -> Result<()> {
        self.apply_provenance(
            decision_id,
            decision,
            content_checksum,
            public_id,
            extracted,
            canonical_ref,
            source_fields,
            payload_format,
        )
        .await
    }

    /// Date du jour les pages dont une couche satellite vient de changer —
    /// références légales, liens de chronologie, citations de jurisprudence,
    /// acteurs. Le rendu a bougé même quand aucune colonne de `decisions` n'a été
    /// touchée.
    async fn touch_content_changed(&self, mut ids: Vec<i64>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        ids.sort_unstable();
        ids.dedup();
        self.conn
            .execute(
                "UPDATE decisions SET updated_at = $1 WHERE id = ANY($2)",
                &[&now(), &ids],
            )
            .await?;
        Ok(())
    }

    /// Copies the facets of `ids` onto their texts, for a writer that runs
    /// with triggers off.
    #[tracing::instrument(name = "db.mirror_text_facets", skip_all, fields(db.system = "postgresql", n = ids.len()))]
    pub async fn mirror_text_facets(&self, ids: &[i64]) -> Result<()> {
        self.conn
            .execute("SELECT decision_text_mirror($1)", &[&ids])
            .await?;
        Ok(())
    }

    #[tracing::instrument(name = "db.update_extracted_fields_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn update_extracted_fields_bulk(
        &self,
        items: &[(i64, ExtractedFields)],
        fields: Option<&[&str]>,
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let selected: Vec<&str> = fields
            .map(|f| f.to_vec())
            .unwrap_or_else(|| REEXTRACTABLE_FIELDS.to_vec());
        let mut column_fields: Vec<&str> = selected
            .iter()
            .copied()
            .filter(|f| !is_routed_field(f))
            .collect();
        if !column_fields.contains(&"formation_paths")
            && column_fields.iter().any(|f| {
                matches!(
                    *f,
                    "seat_paths" | "formation_uid" | "office_uid" | "procedure_uid"
                )
            })
        {
            column_fields.push("formation_paths");
        }

        // Pas de skip-diff colonnes en lecture préalable : le skip vit DANS
        // l'UPDATE (comparaison de tuples, cf. plus bas).
        let column_items: &[(i64, ExtractedFields)] =
            if column_fields.is_empty() { &[] } else { items };

        if !column_items.is_empty() {
            // FK jurisdiction_code : les lignes du référentiel doivent exister avant
            // l'UPDATE (référentiel ouvert) ; la map résout le code source vers le code
            // canonique.
            let jurisdiction_codes = if column_fields.contains(&"jurisdiction_code") {
                let juris: Vec<JurisdictionRow> = column_items
                    .iter()
                    .filter_map(|(_, e)| e.jurisdiction.clone())
                    .collect();
                self.ensure_jurisdictions(&juris).await?
            } else {
                Default::default()
            };
            // Param $1 = updated_at (clause SET avant VALUES).
            let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::new();
            params.push(Box::new(now()));
            let mut idx = 2;

            let mut value_rows: Vec<String> = Vec::with_capacity(column_items.len());
            for (row_pos, (decision_id, extracted)) in column_items.iter().enumerate() {
                let mut cells: Vec<String> = Vec::with_capacity(column_fields.len() + 1);
                // id
                if row_pos == 0 {
                    cells.push(format!("(${idx})::bigint"));
                } else {
                    cells.push(format!("${idx}"));
                }
                params.push(Box::new(*decision_id));
                idx += 1;
                for field in &column_fields {
                    if row_pos == 0 {
                        cells.push(format!("(${})::{}", idx, extracted_column_type(field)));
                    } else {
                        cells.push(format!("${idx}"));
                    }
                    if *field == "jurisdiction_code" {
                        params.push(Box::new(
                            extracted
                                .jurisdiction
                                .as_ref()
                                .and_then(|j| jurisdiction_codes.get(&j.source_code).cloned()),
                        ));
                    } else {
                        params.push(extracted_field_value(extracted, field));
                    }
                    idx += 1;
                }
                value_rows.push(format!("({})", cells.join(", ")));
            }

            // RHS de chaque colonne (réutilisé par le SET et le skip-tuple) : la
            // valeur ré-extraite fait autorité.
            let rhs: Vec<String> = column_fields
                .iter()
                .map(|field| format!("src.{field}"))
                .collect();
            let d_tuple: Vec<String> = column_fields.iter().map(|f| format!("d.{f}")).collect();
            let mut assignments: Vec<String> = Vec::with_capacity(column_fields.len() + 2);
            for (field, rhs) in column_fields.iter().zip(&rhs) {
                assignments.push(format!("{field} = {rhs}"));
            }
            // `updated_at` = date du dernier changement de CONTENU, pas de la dernière
            // écriture : même comparaison NULL-safe que le skip-tuple ci-dessous, portée
            // sur l'affectation. Un bump d'EXTRACT_VERSION réécrit donc la ligne — le
            // tampon de version doit monter — sans avancer la date d'une page inchangée.
            assignments.push(format!(
                "updated_at = CASE WHEN ({}) IS DISTINCT FROM ({}) THEN $1 ELSE d.updated_at END",
                d_tuple.join(", "),
                rhs.join(", "),
            ));

            let cols_decl: Vec<String> = std::iter::once("id".to_string())
                .chain(column_fields.iter().map(|f| (*f).to_string()))
                .collect();

            // Invariant : un écrivain à version V ne remplace jamais des données à
            // version > V (révisions manuelles incluses). Le tampon de version s'écrit dans
            // le MÊME UPDATE (une seule version de ligne par passe — la table porte l'index
            // BM25) : tous les items re-parsés sont à jour, y compris ceux dont aucun champ
            // n'a changé. Ne fait que monter.
            // Skip in-UPDATE : une ligne DÉJÀ à la version courante dont aucune colonne ne
            // changerait n'est pas réécrite (comparaison de tuples NULL-safe).
            params.push(Box::new(EXTRACT_VERSION));
            let sql = format!(
                "UPDATE decisions d SET {}, extract_version = ${idx} \
                 FROM (VALUES {}) AS src({}) \
                 WHERE d.id = src.id \
                   AND (d.extract_version IS NULL OR d.extract_version <= ${idx}) \
                   AND (d.extract_version IS DISTINCT FROM ${idx} \
                     OR ({}) IS DISTINCT FROM ({}))",
                assignments.join(", "),
                value_rows.join(","),
                cols_decl.join(", "),
                d_tuple.join(", "),
                rhs.join(", "),
            );
            let refs = as_param_refs(&params);
            self.conn.execute(sql.as_str(), &refs).await?;
        }

        if selected.contains(&CASE_NAME_FIELD) {
            let names: Vec<(i64, Option<&str>)> = items
                .iter()
                .map(|(id, e)| (*id, e.case_name.as_deref()))
                .collect();
            self.set_source_case_names(&names).await?;
        }

        // Les couches satellites vivent hors de l'UPDATE colonnes : elles datent la
        // page elles-mêmes quand elles changent.
        let mut satellites: Vec<i64> = Vec::new();
        if selected.contains(&LEGAL_REFS_FIELD) {
            let refs_items: Vec<super::citations::CitationWriteItem> = items
                .iter()
                .map(|(id, e)| (*id, e.citation_occurrences.clone()))
                .collect();
            satellites.extend(self.replace_citations_bulk(&refs_items).await?);
        }
        if selected.contains(&DECISION_LINKS_FIELD) {
            let link_items: Vec<super::links::DecisionLinkWriteItem> = items
                .iter()
                .map(|(id, e)| (*id, e.decision_links.clone()))
                .collect();
            satellites.extend(self.replace_decision_links_bulk(&link_items).await?);
        }
        if selected.contains(&CASE_CITATIONS_FIELD) {
            let case_items: Vec<super::cases::CaseCitationWriteItem> = items
                .iter()
                .map(|(id, e)| (*id, e.case_citations.clone()))
                .collect();
            satellites.extend(self.replace_case_citations_bulk(&case_items).await?);
        }
        if selected.contains(&PARTIES_FIELD) {
            let actor_items: Vec<super::parties::DecisionActorWriteItem> = items
                .iter()
                .map(|(id, e)| (*id, e.parties.clone()))
                .collect();
            satellites.extend(self.replace_decision_actors_bulk(&actor_items).await?);
        }
        self.touch_content_changed(satellites).await?;

        // Tampon de version quand aucune colonne n'est sélectionnée (run
        // `--fields legal_references` seul) — sinon il est fusionné dans l'UPDATE
        // colonnes ci-dessus. Ne fait que MONTER : une version supérieure (révision
        // manuelle) n'est jamais dégradée.
        if column_fields.is_empty() {
            let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
            self.conn
                .execute(
                    "UPDATE decisions SET extract_version = $1
                     WHERE id = ANY($2)
                       AND (extract_version IS NULL OR extract_version < $1)",
                    &[&EXTRACT_VERSION, &ids],
                )
                .await?;
        }
        Ok(())
    }

    /// Compte, sans rien écrire, les lignes qu'un `--overwrite` réécrirait —
    /// même source `VALUES` et même comparaison NULL-safe que le skip-tuple de
    /// [`Self::update_extracted_fields_bulk`], en SELECT. Renvoie, aligné sur
    /// les champs colonnes de `fields`, le nombre de lignes divergentes et
    /// jusqu'à deux exemples `#id ancien → nouveau`.
    ///
    /// `jurisdiction_code` se résout en LECTURE (pas d'`ensure_jurisdictions`,
    /// qui crée) : un `source_code` absent du référentiel compte comme
    /// divergence vers `∅`, ce qu'une vraie passe créerait.
    pub async fn diff_extracted_fields_bulk(
        &self,
        items: &[(i64, ExtractedFields)],
        fields: &[&str],
    ) -> Result<Vec<(String, i64, Vec<String>)>> {
        let column_fields: Vec<&str> = fields
            .iter()
            .copied()
            .filter(|f| !is_routed_field(f))
            .collect();
        if items.is_empty() || column_fields.is_empty() {
            return Ok(Vec::new());
        }

        let jurisdiction_codes: std::collections::HashMap<String, String> =
            if column_fields.contains(&"jurisdiction_code") {
                let sources: Vec<String> = items
                    .iter()
                    .filter_map(|(_, e)| e.jurisdiction.as_ref().map(|j| j.source_code.clone()))
                    .collect();
                self.conn
                    .query(
                        "SELECT source_code, code FROM jurisdiction WHERE source_code = ANY($1)",
                        &[&sources],
                    )
                    .await?
                    .into_iter()
                    .map(|r| (r.get(0), r.get(1)))
                    .collect()
            } else {
                Default::default()
            };

        let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        let mut idx = 1;
        let mut value_rows: Vec<String> = Vec::with_capacity(items.len());
        for (row_pos, (decision_id, extracted)) in items.iter().enumerate() {
            let mut cells: Vec<String> = Vec::with_capacity(column_fields.len() + 1);
            if row_pos == 0 {
                cells.push(format!("(${idx})::bigint"));
            } else {
                cells.push(format!("${idx}"));
            }
            params.push(Box::new(*decision_id));
            idx += 1;
            for field in &column_fields {
                if row_pos == 0 {
                    cells.push(format!("(${})::{}", idx, extracted_column_type(field)));
                } else {
                    cells.push(format!("${idx}"));
                }
                if *field == "jurisdiction_code" {
                    params.push(Box::new(
                        extracted
                            .jurisdiction
                            .as_ref()
                            .and_then(|j| jurisdiction_codes.get(&j.source_code).cloned()),
                    ));
                } else {
                    params.push(extracted_field_value(extracted, field));
                }
                idx += 1;
            }
            value_rows.push(format!("({})", cells.join(", ")));
        }

        let mut projections: Vec<String> = Vec::with_capacity(column_fields.len() * 2);
        for field in &column_fields {
            projections.push(format!(
                "count(*) FILTER (WHERE d.{field} IS DISTINCT FROM src.{field})"
            ));
            projections.push(format!(
                "(array_agg(format('#%s %s → %s', d.id, \
                   coalesce(d.{field}::text, '∅'), coalesce(src.{field}::text, '∅'))) \
                 FILTER (WHERE d.{field} IS DISTINCT FROM src.{field}))[1:2]"
            ));
        }
        let cols_decl: Vec<String> = std::iter::once("id".to_string())
            .chain(column_fields.iter().map(|f| (*f).to_string()))
            .collect();
        let sql = format!(
            "SELECT {} FROM (VALUES {}) AS src({}) JOIN decisions d ON d.id = src.id",
            projections.join(", "),
            value_rows.join(","),
            cols_decl.join(", "),
        );
        let refs = as_param_refs(&params);
        let row = self.conn.query_one(sql.as_str(), &refs).await?;
        Ok(column_fields
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let samples: Option<Vec<String>> = row.get(2 * i + 1);
                (
                    (*field).to_string(),
                    row.get::<_, i64>(2 * i),
                    samples.unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Crée une décision (nouvelle identité) : INSERT du canonique pur (ni
    /// `source_uid`/`content_checksum`/`source_fields` — ils vivent sur
    /// `decision_sources`), avec `canonical_ref`, puis enregistre la provenance (qui
    /// porte `source_fields`). Nouvelle décision → autorité triviale, son texte est
    /// canonique.
    #[allow(clippy::too_many_arguments)]
    async fn insert_decision(
        &self,
        decision: &Decision,
        content_checksum: &str,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        source_fields: &Value,
        payload_format: &str,
    ) -> Result<i64> {
        let e = extracted;
        let jurisdiction_code = match e.and_then(|e| e.jurisdiction.clone()) {
            Some(j) => self
                .ensure_jurisdictions(std::slice::from_ref(&j))
                .await?
                .remove(&j.source_code),
            None => None,
        };
        let case_name = e.and_then(|e| e.case_name.clone());
        let case_name_keys = case_name
            .as_deref()
            .map(|n| vec![lj_core::case_names::case_name_key(n)]);
        let case_name_source = case_name.as_ref().map(|_| "source");
        let lang = original_lang(source_fields)?;
        let row = self
            .conn
            .query_one(
                "
                INSERT INTO decisions (
                  jurisdiction_type, public_id, updated_at,
                  date_lecture, date_audience, docket_numbers,
                  publication_codes, extract_version,
                  ecli, canonical_ref,
                  solution_uid, procedure_uid, office_uid, legal_domain_uid,
                  jurisdiction_code, themes, formation_uid,
                  search_title, case_name, case_name_keys, case_name_source,
                  seat, seat_paths, lang, formation_paths, kind
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                        $17, $18, $19, $20, $21, $22, $23, $24, $25, $26)
                RETURNING id
                ",
                &[
                    &decision.jurisdiction_type,
                    &public_id,
                    &now(),
                    &e.and_then(|e| e.date_lecture),
                    &e.and_then(|e| e.date_audience),
                    &e.map(|e| e.docket_numbers.clone()).unwrap_or_default(),
                    &e.map(|e| e.publication_codes.clone()).unwrap_or_default(),
                    &EXTRACT_VERSION,
                    &decision.ecli,
                    &canonical_ref,
                    &e.and_then(|e| e.solution_uid.clone()),
                    &e.and_then(|e| e.procedure_uid.clone()),
                    &e.and_then(|e| e.office_uid.clone()),
                    &e.and_then(|e| e.legal_domain_uid.clone()),
                    &jurisdiction_code,
                    &e.map(|e| e.themes.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.formation_uid.clone()),
                    &e.and_then(|e| e.search_title.clone()),
                    &case_name,
                    &case_name_keys,
                    &case_name_source,
                    &e.and_then(|e| e.seat.clone()).map(Json),
                    &e.map(|e| e.seat_paths.clone()).unwrap_or_default(),
                    &lang,
                    &e.map(formation_paths).unwrap_or_default(),
                    &e.map(|e| e.kind).unwrap_or_default().as_str(),
                ],
            )
            .await?;
        let decision_id: i64 = row.get(0);
        if let Some(e) = e {
            self.replace_citations(decision_id, e.citation_occurrences.as_deref())
                .await?;
            self.replace_decision_links(decision_id, e.decision_links.as_deref())
                .await?;
            self.replace_case_citations(decision_id, e.case_citations.as_deref())
                .await?;
            self.replace_decision_actors(decision_id, e.parties.as_deref())
                .await?;
        }
        self.upsert_decision_source(
            decision_id,
            &decision.source_uid,
            content_checksum,
            payload_format,
            source_fields,
        )
        .await?;
        self.put_text(
            decision_id,
            &decision.source_uid,
            source_fields,
            &decision.texte_integral_clean,
        )
        .await?;
        Ok(decision_id)
    }

    /// Pagination keyset sur `id` : `limit` lignes `(id, full_text)` d'`id` >
    /// `after_id`. L'appelant avance `after_id` jusqu'à un batch vide.
    /// `sample_mod` ≥ 1 sous-échantillonne (`1` = tout le corpus).
    #[tracing::instrument(name = "db.fetch_chunk_bodies_after", skip(self), fields(db.system = "postgresql"))]
    pub async fn fetch_chunk_bodies_after(
        &self,
        after_id: i64,
        sample_mod: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT decision_id, full_text FROM decision_text \
                 WHERE decision_id > $1 AND decision_id % $2 = 0 AND {read} \
                 ORDER BY decision_id LIMIT $3",
                    read = super::pick::read("decision_text")
                ),
                &[&after_id, &sample_mod, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }
}

/// Canonical code of a court: TJ, TCOM and CPH take the slug of their city
/// (`tj_paris`, `tcom_lyon`, `cph_lorient`); any other type keeps its source code
/// (singletons `cc`/`ce`, and `ca_`/`caa_`/`ta_` slugs already named by city).
/// Without a usable city, the source code.
pub fn canonical_jurisdiction_code(
    jurisdiction_type: &str,
    city: Option<&str>,
    source_code: &str,
) -> String {
    if !matches!(jurisdiction_type, "TJ" | "TCOM" | "CPH") {
        return source_code.to_string();
    }
    match city
        .map(|c| lj_fold::slug(c, '_'))
        .filter(|s| !s.is_empty())
    {
        Some(slug) => format!("{}_{slug}", jurisdiction_type.to_lowercase()),
        None => source_code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_codes_slug_tj_tcom_cities() {
        for (city, expected) in [
            ("Paris", "tj_paris"),
            ("Saint-Étienne", "tj_saint_etienne"),
            ("Épinal", "tj_epinal"),
            ("Châlons-en-Champagne", "tj_chalons_en_champagne"),
            ("Les Sables-d'Olonne", "tj_les_sables_d_olonne"),
            ("Saint-Denis de la Réunion", "tj_saint_denis_de_la_reunion"),
            ("Besançon", "tj_besancon"),
            ("Alençon", "tj_alencon"),
            ("Montluçon", "tj_montlucon"),
            ("Le Havre", "tj_le_havre"),
        ] {
            assert_eq!(
                canonical_jurisdiction_code("TJ", Some(city), "tjXXXXX"),
                expected
            );
        }
        assert_eq!(
            canonical_jurisdiction_code(
                "TCOM",
                Some("Villefranche-sur-Saône - Tarare"),
                "tcom6903"
            ),
            "tcom_villefranche_sur_saone_tarare"
        );
        assert_eq!(
            canonical_jurisdiction_code("CPH", Some("Bourgoin-Jallieu"), "cph101036"),
            "cph_bourgoin_jallieu"
        );
        assert_eq!(
            canonical_jurisdiction_code("TJ", None, "tj75056"),
            "tj75056"
        );
        assert_eq!(
            canonical_jurisdiction_code("CA", Some("Paris"), "ca_paris"),
            "ca_paris"
        );
        assert_eq!(canonical_jurisdiction_code("CC", None, "cc"), "cc");
    }
}
