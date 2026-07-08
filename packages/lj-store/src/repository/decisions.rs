//! Upsert des métadonnées décision et écriture du contenu canonique (ADR 0098) :
//! `upsert`/`update_existing`, application de provenance, écriture canonique,
//! création, et mise à jour batchée des champs ré-extractibles.

use super::support::{
    as_param_refs, extracted_column_type, extracted_field_value, now, CASE_CITATIONS_FIELD,
    DECISION_LINKS_FIELD, LEGAL_REFS_FIELD, REEXTRACTABLE_FIELDS,
};
use super::types::{ExtractedFields, JurisdictionRow, UpsertResult, UpsertStatus};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use lj_core::decision::Decision;
use lj_core::EXTRACT_VERSION;
use serde_json::Value;
use tokio_postgres::types::ToSql;

impl DecisionRepository<'_> {
    #[tracing::instrument(name = "db.count_decisions", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_decisions(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one("SELECT COUNT(*) FROM decisions", &[])
            .await?;
        Ok(row.get::<_, i64>(0))
    }

    /// Nombre de décisions **actives** (non soft-deleted) = corpus réellement
    /// indexé et cherchable. Compte exact plutôt que l'estimation `pg_class.
    /// reltuples`, qui sur-estime de plusieurs centaines de milliers après le
    /// bloat des UPDATE de ré-extraction ; et `reltuples` ne peut de toute façon
    /// pas filtrer `deleted_at`. Servi via cache 12 h (stats accueil) → un seq
    /// scan 2×/jour est négligeable.
    #[tracing::instrument(name = "db.count_active_decisions", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_active_decisions(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*) FROM decisions WHERE deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(row.get::<_, i64>(0))
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
        embed_version: Option<i16>,
        payload_format: &str,
    ) -> Result<UpsertResult> {
        if decision.juridiction_type.is_none() {
            return Err(StoreError::Invalid(format!(
                "Décision {} : juridiction_type inconnue, refus d'insertion (contrainte NOT NULL).",
                decision.source_uid
            )));
        }

        // ADR 0098 §4 : l'upsert est **piloté par `source_uid`** (pivot
        // d'idempotence sur `decision_sources`).
        if let Some((id, stored_checksum, active)) =
            self.find_provenance(&decision.source_uid).await?
        {
            // Provenance connue (active OU tombstonée) → ré-update en place, même
            // décision (§4.1). Skip seulement si active ET checksum inchangé ;
            // une provenance tombstonée se ressuscite même à checksum égal.
            if active && stored_checksum == content_checksum {
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
                embed_version,
                payload_format,
            )
            .await?;
            return Ok(UpsertResult {
                id,
                status: UpsertStatus::Updated,
            });
        }

        // `source_uid` inconnu → résolution d'identité : ECLI puis `canonical_ref`
        // (ADR 0100). Match → on attache la provenance à la décision trouvée ;
        // sinon → création.
        if let Some(existing_id) = self.resolve_identity(decision, canonical_ref).await? {
            self.apply_provenance(
                existing_id,
                decision,
                content_checksum,
                public_id,
                extracted,
                canonical_ref,
                source_fields,
                embed_version,
                payload_format,
            )
            .await?;
            return Ok(UpsertResult {
                id: existing_id,
                status: UpsertStatus::Updated,
            });
        }

        let id = self
            .insert_decision(
                decision,
                content_checksum,
                public_id,
                extracted,
                canonical_ref,
                source_fields,
                embed_version,
                payload_format,
            )
            .await?;
        Ok(UpsertResult {
            id,
            status: UpsertStatus::Created,
        })
    }

    /// Applique la provenance entrante à une décision EXISTANTE `decision_id`
    /// (ADR 0098 §3/§4), sous verrou xact sur la décision. (1) upsert de la
    /// `decision_sources` (porte `source_fields`, lève le tombstone) ; (2)
    /// réécrit le contenu canonique (`full_text`/métadonnées) **SSI l'entrant est
    /// (ou devient) l'autorité** — provenance de rang max (§3) : une source de
    /// rang inférieur attache ses méta sans toucher au texte servi (garde RGPD) ;
    /// (3) `reconcile` (état `deleted_at` / vide).
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
        embed_version: Option<i16>,
        payload_format: &str,
    ) -> Result<()> {
        // Verrou par décision (relâché en fin de transaction de l'appelant) :
        // sérialise upsert et retrait concurrents sur la même décision (§4).
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
        // 2) Contenu canonique : seule l'autorité (rang max, §3/§4.2) écrit
        //    `full_text`/métadonnées. La provenance vient d'être upsertée, donc
        //    elle entre dans le calcul de l'autorité.
        if self.authority_source_uid(decision_id).await?.as_deref()
            == Some(decision.source_uid.as_str())
        {
            self.write_canonical_content(
                decision_id,
                decision,
                public_id,
                extracted,
                canonical_ref,
                embed_version,
            )
            .await?;
            if let Some(e) = extracted {
                self.replace_citations(decision_id, e.citation_occurrences.as_deref())
                    .await?;
                self.replace_decision_links(decision_id, e.decision_links.as_deref())
                    .await?;
                self.replace_case_citations(decision_id, e.case_citations.as_deref())
                    .await?;
            }
        }
        // 3) Reconcile : `deleted_at` / vide RGPD (no-op si autorité active).
        self.reconcile(decision_id).await?;
        Ok(())
    }

    /// Réécrit les colonnes **canoniques** d'une décision depuis sa provenance
    /// autoritaire (ADR 0098 §3) : métadonnées coalescées, `full_text` (texte de
    /// l'autorité), identité (`ecli`/`canonical_ref`), versions. N'écrit **rien**
    /// de spécifique à une provenance (`source_uid`/`content_checksum`/
    /// `source_fields` vivent sur `decision_sources`). No-op sur une décision à
    /// `extract_version` > `EXTRACT_VERSION` (ADR 0140 : un écrivain à
    /// version V ne remplace jamais des données à version > V).
    async fn write_canonical_content(
        &self,
        decision_id: i64,
        decision: &Decision,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        embed_version: Option<i16>,
    ) -> Result<()> {
        let e = extracted;
        if let Some(j) = e.and_then(|e| e.jurisdiction.clone()) {
            self.ensure_jurisdictions(&[j]).await?;
        }
        self.conn
            .execute(
                "
                UPDATE decisions SET
                  juridiction_type = $1,
                  public_id = $2, updated_at = $3,
                  jurisdiction_name = $4,
                  date_lecture = $5, date_audience = $6, docket_numbers = $7,
                  formation_or_chamber = $8,
                  publication_codes = $9,
                  extract_version = $11,
                  full_text = $12, embed_version = $13, ecli = $14, canonical_ref = $15,
                  solution_uid = $16, voie_uid = $17, office_uid = $18,
                  legal_domain_uid = $19, publication_uid = $20,
                  jurisdiction_code = $21,
                  applicant_counsel_names = $22, applicant_law_firms = $23,
                  applicant_companies = $24, defendant_counsel_names = $25,
                  defendant_law_firms = $26, defendant_companies = $27,
                  themes = $28,
                  chamber_position = $29, chambre_uid = $30, formation_uid = $31
                WHERE id = $10
                  AND (extract_version IS NULL OR extract_version <= $11)
                ",
                &[
                    &decision.juridiction_type,
                    &public_id,
                    &now(),
                    &e.and_then(|e| e.jurisdiction_name.clone()),
                    &e.and_then(|e| e.date_lecture),
                    &e.and_then(|e| e.date_audience),
                    &e.map(|e| e.docket_numbers.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.formation_or_chamber.clone()),
                    &e.map(|e| e.publication_codes.clone()).unwrap_or_default(),
                    &decision_id,
                    &EXTRACT_VERSION,
                    &decision.texte_integral_clean,
                    &embed_version,
                    &decision.ecli,
                    &canonical_ref,
                    &e.and_then(|e| e.solution_uid.clone()),
                    &e.and_then(|e| e.voie_uid.clone()),
                    &e.and_then(|e| e.office_uid.clone()),
                    &e.and_then(|e| e.legal_domain_uid.clone()),
                    &e.and_then(|e| e.publication_uid.clone()),
                    &e.and_then(|e| e.jurisdiction.as_ref().map(|j| j.code.clone())),
                    &e.map(|e| e.applicant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.applicant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.applicant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.defendant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.themes.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.chamber_position.clone()),
                    &e.and_then(|e| e.chambre_uid.clone()),
                    &e.and_then(|e| e.formation_uid.clone()),
                ],
            )
            .await?;
        Ok(())
    }

    /// Crée les lignes du référentiel `jurisdiction` manquantes (ADR 0146) —
    /// référentiel OUVERT nourri par la donnée, contrairement à `facet_value`
    /// (vocabulaire fermé seedé). Conflit sur `code` : la ligne existante ne
    /// bouge que si la nouvelle est plus disante (elle apporte la ville et
    /// l'existante n'en a pas) — un libellé source nu (« Tribunal
    /// judiciaire ») ne dégrade jamais un label complet, et une ligne née nue
    /// guérit dès qu'une décision du même code porte la ville.
    pub async fn ensure_jurisdictions(&self, rows: &[JurisdictionRow]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        // Dédup par code en ordre TRIÉ : l'upsert verrouille ses lignes dans
        // l'ordre du VALUES et les tient jusqu'au COMMIT du lot appelant — un
        // ordre non déterministe entre workers reextract concurrents a produit
        // un vrai deadlock (2026-07-05, deux lots aux codes croisés).
        let mut by_code: std::collections::BTreeMap<&str, &JurisdictionRow> =
            std::collections::BTreeMap::new();
        for r in rows {
            by_code.entry(r.code.as_str()).or_insert(r);
        }
        // Pré-filtre lecture seule : en régime établi le référentiel porte déjà
        // tous les codes avec ville — `ON CONFLICT DO UPDATE` verrouillerait
        // pourtant CHAQUE ligne conflictuelle (avant d'évaluer son WHERE), et
        // les workers se sérialiseraient sur les codes chauds. On n'upserte que
        // les codes absents ou guérissables ; la course résiduelle (deux
        // workers découvrent le même code neuf) est absorbée par l'ON CONFLICT.
        let all_codes: Vec<&str> = by_code.keys().copied().collect();
        let existing: std::collections::HashMap<String, bool> = self
            .conn
            .query(
                "SELECT code, city IS NOT NULL FROM jurisdiction WHERE code = ANY($1)",
                &[&all_codes],
            )
            .await?
            .into_iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        let (mut codes, mut types, mut cities, mut labels) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (code, r) in by_code {
            match existing.get(code) {
                Some(true) => continue,                      // complète
                Some(false) if r.city.is_none() => continue, // rien à guérir
                _ => {}
            }
            codes.push(r.code.clone());
            types.push(r.juridiction_type.clone());
            cities.push(r.city.clone());
            labels.push(r.label.clone());
        }
        if codes.is_empty() {
            return Ok(());
        }
        self.conn
            .execute(
                "INSERT INTO jurisdiction (code, juridiction_type, city, label)
                 SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[])
                 ON CONFLICT (code) DO UPDATE
                     SET city = EXCLUDED.city, label = EXCLUDED.label
                 WHERE jurisdiction.city IS NULL AND EXCLUDED.city IS NOT NULL",
                &[&codes, &types, &cities, &labels],
            )
            .await?;
        Ok(())
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
    /// via [`apply_provenance`](Self::apply_provenance) (ADR 0098 §4) — comme
    /// `upsert` côté décision existante, mais sans re-sonder l'identité.
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
        embed_version: Option<i16>,
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
            embed_version,
            payload_format,
        )
        .await
    }

    #[tracing::instrument(name = "db.update_extracted_fields", skip(self, extracted), fields(db.system = "postgresql"))]
    pub async fn update_extracted_fields(
        &self,
        decision_id: i64,
        extracted: &ExtractedFields,
        fields: Option<&[&str]>,
        overwrite: bool,
    ) -> Result<()> {
        let selected: Vec<&str> = fields
            .map(|f| f.to_vec())
            .unwrap_or_else(|| REEXTRACTABLE_FIELDS.to_vec());
        let column_fields: Vec<&str> = selected
            .iter()
            .copied()
            .filter(|f| {
                *f != LEGAL_REFS_FIELD && *f != DECISION_LINKS_FIELD && *f != CASE_CITATIONS_FIELD
            })
            .collect();

        if !column_fields.is_empty() {
            let mut assignments: Vec<String> = Vec::with_capacity(column_fields.len() + 1);
            let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::new();
            let mut idx = 1;
            for field in &column_fields {
                if overwrite {
                    assignments.push(format!("{field} = ${idx}"));
                } else {
                    assignments.push(format!("{field} = COALESCE({field}, ${idx})"));
                }
                params.push(extracted_field_value(extracted, field));
                idx += 1;
            }
            assignments.push(format!("updated_at = ${idx}"));
            params.push(Box::new(now()));
            idx += 1;
            let id_ph = idx;
            params.push(Box::new(decision_id));

            let sql = format!(
                "UPDATE decisions SET {} WHERE id = ${}",
                assignments.join(", "),
                id_ph,
            );
            let refs = as_param_refs(&params);
            self.conn.execute(sql.as_str(), &refs).await?;
        }

        if selected.contains(&LEGAL_REFS_FIELD)
            && (overwrite || extracted.citation_occurrences.is_some())
        {
            self.replace_citations(decision_id, extracted.citation_occurrences.as_deref())
                .await?;
        }
        if selected.contains(&DECISION_LINKS_FIELD)
            && (overwrite || extracted.decision_links.is_some())
        {
            self.replace_decision_links(decision_id, extracted.decision_links.as_deref())
                .await?;
        }
        if selected.contains(&CASE_CITATIONS_FIELD)
            && (overwrite || extracted.case_citations.is_some())
        {
            self.replace_case_citations(decision_id, extracted.case_citations.as_deref())
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(name = "db.update_extracted_fields_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn update_extracted_fields_bulk(
        &self,
        items: &[(i64, ExtractedFields)],
        fields: Option<&[&str]>,
        overwrite: bool,
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let selected: Vec<&str> = fields
            .map(|f| f.to_vec())
            .unwrap_or_else(|| REEXTRACTABLE_FIELDS.to_vec());
        let column_fields: Vec<&str> = selected
            .iter()
            .copied()
            .filter(|f| {
                *f != LEGAL_REFS_FIELD && *f != DECISION_LINKS_FIELD && *f != CASE_CITATIONS_FIELD
            })
            .collect();

        // Pas de skip-diff colonnes en lecture préalable : le skip vit DANS
        // l'UPDATE (comparaison de tuples, cf. plus bas) — l'ancien diff
        // relisait toutes les colonnes ré-extractibles du lot.
        let column_items: &[(i64, ExtractedFields)] =
            if column_fields.is_empty() { &[] } else { items };

        if !column_items.is_empty() {
            // FK jurisdiction_code : les lignes du référentiel doivent exister
            // avant l'UPDATE (référentiel ouvert, ADR 0146).
            if column_fields.contains(&"jurisdiction_code") {
                let juris: Vec<JurisdictionRow> = column_items
                    .iter()
                    .filter_map(|(_, e)| e.jurisdiction.clone())
                    .collect();
                self.ensure_jurisdictions(&juris).await?;
            }
            // Param $1 = updated_at (clause SET avant VALUES, cf. Python).
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
                    params.push(extracted_field_value(extracted, field));
                    idx += 1;
                }
                value_rows.push(format!("({})", cells.join(", ")));
            }

            // RHS de chaque colonne (réutilisé par le SET et le skip-tuple) :
            // overwrite = vérité de l'extracteur ; sinon fill-des-NULLs.
            let rhs: Vec<String> = column_fields
                .iter()
                .map(|field| {
                    if overwrite {
                        format!("src.{field}")
                    } else {
                        format!("COALESCE(d.{field}, src.{field})")
                    }
                })
                .collect();
            let mut assignments: Vec<String> = Vec::with_capacity(column_fields.len() + 2);
            for (field, rhs) in column_fields.iter().zip(&rhs) {
                assignments.push(format!("{field} = {rhs}"));
            }
            assignments.push("updated_at = $1".to_string());

            let cols_decl: Vec<String> = std::iter::once("id".to_string())
                .chain(column_fields.iter().map(|f| (*f).to_string()))
                .collect();

            // Invariant : un écrivain à version V ne remplace jamais des données
            // à version > V (révisions manuelles incluses, ADR 0140). Le tampon
            // de version s'écrit dans le MÊME UPDATE (une seule version de
            // ligne par passe — la table porte l'index BM25) : tous les items
            // re-parsés sont à jour, y compris ceux dont aucun champ n'a
            // changé (reprise du reextract, ADR 0083). Ne fait que monter.
            // Skip in-UPDATE : une ligne DÉJÀ à la version courante dont aucune
            // colonne ne changerait n'est pas réécrite (comparaison de tuples
            // NULL-safe) — la passe intégrale hebdomadaire (cron `--full`)
            // rejoue tout le fonds et ne doit churner ni lignes ni index BM25.
            params.push(Box::new(EXTRACT_VERSION));
            let d_tuple: Vec<String> = column_fields.iter().map(|f| format!("d.{f}")).collect();
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

        if selected.contains(&LEGAL_REFS_FIELD) {
            let refs_items: Vec<super::citations::CitationWriteItem> = items
                .iter()
                .filter(|(_, e)| overwrite || e.citation_occurrences.is_some())
                .map(|(id, e)| (*id, e.citation_occurrences.clone()))
                .collect();
            self.replace_citations_bulk(&refs_items).await?;
        }
        if selected.contains(&DECISION_LINKS_FIELD) {
            let link_items: Vec<super::links::DecisionLinkWriteItem> = items
                .iter()
                .filter(|(_, e)| overwrite || e.decision_links.is_some())
                .map(|(id, e)| (*id, e.decision_links.clone()))
                .collect();
            self.replace_decision_links_bulk(&link_items).await?;
        }
        if selected.contains(&CASE_CITATIONS_FIELD) {
            let case_items: Vec<super::cases::CaseCitationWriteItem> = items
                .iter()
                .filter(|(_, e)| overwrite || e.case_citations.is_some())
                .map(|(id, e)| (*id, e.case_citations.clone()))
                .collect();
            self.replace_case_citations_bulk(&case_items).await?;
        }

        // Tampon de version quand aucune colonne n'est sélectionnée (run
        // `--fields legal_references` seul) — sinon il est fusionné dans
        // l'UPDATE colonnes ci-dessus. Ne fait que MONTER : une version
        // supérieure (révision manuelle, ADR 0140) n'est jamais dégradée.
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

    /// Crée une décision (nouvelle identité, §4.2) : INSERT du canonique pur (ni
    /// `source_uid`/`content_checksum`/`source_fields` — ils vivent sur
    /// `decision_sources`), avec `canonical_ref` (ADR 0100), puis enregistre la
    /// provenance (qui porte `source_fields`). Nouvelle décision → autorité
    /// triviale, son texte est canonique.
    #[allow(clippy::too_many_arguments)]
    async fn insert_decision(
        &self,
        decision: &Decision,
        content_checksum: &str,
        public_id: &str,
        extracted: Option<&ExtractedFields>,
        canonical_ref: Option<&str>,
        source_fields: &Value,
        embed_version: Option<i16>,
        payload_format: &str,
    ) -> Result<i64> {
        let e = extracted;
        if let Some(j) = e.and_then(|e| e.jurisdiction.clone()) {
            self.ensure_jurisdictions(&[j]).await?;
        }
        let row = self
            .conn
            .query_one(
                "
                INSERT INTO decisions (
                  juridiction_type, public_id, updated_at,
                  jurisdiction_name,
                  date_lecture, date_audience, docket_numbers,
                  formation_or_chamber, publication_codes, extract_version,
                  full_text, embed_version, ecli, canonical_ref,
                  solution_uid, voie_uid, office_uid, legal_domain_uid,
                  publication_uid, jurisdiction_code,
                  applicant_counsel_names, applicant_law_firms, applicant_companies,
                  defendant_counsel_names, defendant_law_firms, defendant_companies,
                  themes, chamber_position, chambre_uid, formation_uid
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                        $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30)
                RETURNING id
                ",
                &[
                    &decision.juridiction_type,
                    &public_id,
                    &now(),
                    &e.and_then(|e| e.jurisdiction_name.clone()),
                    &e.and_then(|e| e.date_lecture),
                    &e.and_then(|e| e.date_audience),
                    &e.map(|e| e.docket_numbers.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.formation_or_chamber.clone()),
                    &e.map(|e| e.publication_codes.clone()).unwrap_or_default(),
                    &EXTRACT_VERSION,
                    &decision.texte_integral_clean,
                    &embed_version,
                    &decision.ecli,
                    &canonical_ref,
                    &e.and_then(|e| e.solution_uid.clone()),
                    &e.and_then(|e| e.voie_uid.clone()),
                    &e.and_then(|e| e.office_uid.clone()),
                    &e.and_then(|e| e.legal_domain_uid.clone()),
                    &e.and_then(|e| e.publication_uid.clone()),
                    &e.and_then(|e| e.jurisdiction.as_ref().map(|j| j.code.clone())),
                    &e.map(|e| e.applicant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.applicant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.applicant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.defendant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.themes.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.chamber_position.clone()),
                    &e.and_then(|e| e.chambre_uid.clone()),
                    &e.and_then(|e| e.formation_uid.clone()),
                ],
            )
            .await?;
        let decision_id: i64 = row.get(0);
        if let Some(e) = e {
            self.replace_citations(decision_id, e.citation_occurrences.as_deref())
                .await?;
            self.replace_decision_links(decision_id, e.decision_links.as_deref())
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
        Ok(decision_id)
    }

    /// Streame le texte des décisions par pagination keyset sur `id` : renvoie
    /// jusqu'à `limit` lignes `(id, full_text)` d'`id` strictement > `after_id`,
    /// ordonnées par `id`. L'appelant boucle en avançant `after_id` au dernier
    /// id reçu jusqu'à un batch vide. `sample_mod` ≥ 1 sous-échantillonne
    /// (`id % sample_mod = 0` ; `1` = tout le corpus). Alimente le mineur de
    /// collocations (`lj-bench mine-collocations`) — grain décision (ADR 0084).
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
                "SELECT id, full_text FROM decisions \
                 WHERE id > $1 AND id % $2 = 0 AND full_text IS NOT NULL \
                 ORDER BY id LIMIT $3",
                &[&after_id, &sample_mod, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
            .collect())
    }
}
