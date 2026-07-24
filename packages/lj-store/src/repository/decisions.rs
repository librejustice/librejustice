//! Upsert des métadonnées décision et écriture du contenu canonique (ADR 0098) :
//! `upsert`/`update_existing`, application de provenance, écriture canonique,
//! création, et mise à jour batchée des champs ré-extractibles.

use super::support::{
    as_param_refs, extracted_column_type, extracted_field_value, now, CASE_CITATIONS_FIELD,
    DECISION_LINKS_FIELD, LEGAL_REFS_FIELD, PARTIES_FIELD, REEXTRACTABLE_FIELDS,
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
        if decision.jurisdiction_type.is_none() {
            return Err(StoreError::Invalid(format!(
                "Décision {} : jurisdiction_type inconnue, refus d'insertion (contrainte NOT NULL).",
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
                self.replace_decision_parties(decision_id, e.parties.as_deref())
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
        let jurisdiction_code = match e.and_then(|e| e.jurisdiction.clone()) {
            Some(j) => self
                .ensure_jurisdictions(std::slice::from_ref(&j))
                .await?
                .remove(&j.source_code),
            None => None,
        };
        self.conn
            .execute(
                "
                UPDATE decisions SET
                  jurisdiction_type = $1,
                  public_id = $2, updated_at = $3,
                  date_lecture = $4, date_audience = $5, docket_numbers = $6,
                  publication_codes = $7,
                  extract_version = $9,
                  full_text = $10, embed_version = $11, ecli = $12, canonical_ref = $13,
                  solution_uid = $14, procedure_uid = $15, office_uid = $16,
                  legal_domain_uid = $17, publication_uid = $18,
                  jurisdiction_code = $19,
                  applicant_counsel_names = $20, applicant_law_firms = $21,
                  applicant_companies = $22, defendant_counsel_names = $23,
                  defendant_law_firms = $24, defendant_companies = $25,
                  intervenors = $26, themes = $27,
                  chamber_position = $28, chamber_uid = $29, formation_uid = $30,
                  search_title = $31
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
                    &decision.texte_integral_clean,
                    &embed_version,
                    &decision.ecli,
                    &canonical_ref,
                    &e.and_then(|e| e.solution_uid.clone()),
                    &e.and_then(|e| e.procedure_uid.clone()),
                    &e.and_then(|e| e.office_uid.clone()),
                    &e.and_then(|e| e.legal_domain_uid.clone()),
                    &e.and_then(|e| e.publication_uid.clone()),
                    &jurisdiction_code,
                    &e.map(|e| e.applicant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.applicant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.applicant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.defendant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.intervenors.clone()).unwrap_or_default(),
                    &e.map(|e| e.themes.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.chamber_position.clone()),
                    &e.and_then(|e| e.chamber_uid.clone()),
                    &e.and_then(|e| e.formation_uid.clone()),
                    &e.and_then(|e| e.search_title.clone()),
                ],
            )
            .await?;
        Ok(())
    }

    /// Crée les lignes du référentiel `jurisdiction` manquantes (ADR 0146) —
    /// référentiel OUVERT nourri par la donnée, contrairement à `facet_value`
    /// (vocabulaire fermé seedé). L'entrée est keyée par `source_code` (code de
    /// la source, ex. location Judilibre `tj75056`) ; le code canonique
    /// (`tj_paris`, ADR 0201) est calculé à la création. Renvoie la map
    /// `source_code` → code canonique pour `decisions.jurisdiction_code`.
    /// Conflit sur `source_code` : la ligne existante ne bouge que si la
    /// nouvelle est plus disante (elle apporte la ville et l'existante n'en a
    /// pas) — un libellé source nu (« Tribunal judiciaire ») ne dégrade jamais
    /// un label complet, et une ligne née nue guérit dès qu'une décision du
    /// même code porte la ville (son `code`, lui, reste celui de la création).
    pub async fn ensure_jurisdictions(
        &self,
        rows: &[JurisdictionRow],
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut resolved = std::collections::HashMap::new();
        if rows.is_empty() {
            return Ok(resolved);
        }
        // Dédup par code en ordre TRIÉ : l'upsert verrouille ses lignes dans
        // l'ordre du VALUES et les tient jusqu'au COMMIT du lot appelant — un
        // ordre non déterministe entre workers reextract concurrents a produit
        // un vrai deadlock (2026-07-05, deux lots aux codes croisés).
        let mut by_code: std::collections::BTreeMap<&str, &JurisdictionRow> =
            std::collections::BTreeMap::new();
        for r in rows {
            by_code.entry(r.source_code.as_str()).or_insert(r);
        }
        // Pré-filtre lecture seule : en régime établi le référentiel porte déjà
        // tous les codes avec ville — `ON CONFLICT DO UPDATE` verrouillerait
        // pourtant CHAQUE ligne conflictuelle (avant d'évaluer son WHERE), et
        // les workers se sérialiseraient sur les codes chauds. On n'upserte que
        // les codes absents ou guérissables ; la course résiduelle (deux
        // workers découvrent le même code neuf) est absorbée par l'ON CONFLICT.
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
        // Codes canoniques des sources nouvelles : un code déjà porté par une
        // AUTRE ligne est une variante de nom de la même cour (« TA de St
        // Barthélemy » émis quand la ligne canonique dit « Saint-Barthélemy »)
        // — on résout vers cette ligne, le PK `code` interdit une seconde.
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
        let (mut codes, mut sources, mut types, mut cities, mut labels) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
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
        }
        if codes.is_empty() {
            return Ok(resolved);
        }
        self.conn
            .execute(
                "INSERT INTO jurisdiction (code, source_code, jurisdiction_type, city, label)
                 SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[])
                 ON CONFLICT (source_code) DO UPDATE
                     SET city = EXCLUDED.city, label = EXCLUDED.label
                 WHERE jurisdiction.city IS NULL AND EXCLUDED.city IS NOT NULL",
                &[&codes, &sources, &types, &cities, &labels],
            )
            .await?;
        Ok(resolved)
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
                *f != LEGAL_REFS_FIELD
                    && *f != DECISION_LINKS_FIELD
                    && *f != CASE_CITATIONS_FIELD
                    && *f != PARTIES_FIELD
            })
            .collect();

        if !column_fields.is_empty() {
            // FK jurisdiction_code : ligne référentielle garantie + code source
            // résolu en code canonique (ADR 0201) avant l'UPDATE.
            let jurisdiction_codes = if column_fields.contains(&"jurisdiction_code") {
                let juris: Vec<JurisdictionRow> =
                    extracted.jurisdiction.clone().into_iter().collect();
                self.ensure_jurisdictions(&juris).await?
            } else {
                Default::default()
            };
            let mut assignments: Vec<String> = Vec::with_capacity(column_fields.len() + 1);
            let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::new();
            let mut idx = 1;
            for field in &column_fields {
                if overwrite {
                    assignments.push(format!("{field} = ${idx}"));
                } else {
                    assignments.push(format!("{field} = COALESCE({field}, ${idx})"));
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
        if selected.contains(&PARTIES_FIELD) && (overwrite || extracted.parties.is_some()) {
            self.replace_decision_parties(decision_id, extracted.parties.as_deref())
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
                *f != LEGAL_REFS_FIELD
                    && *f != DECISION_LINKS_FIELD
                    && *f != CASE_CITATIONS_FIELD
                    && *f != PARTIES_FIELD
            })
            .collect();

        // Pas de skip-diff colonnes en lecture préalable : le skip vit DANS
        // l'UPDATE (comparaison de tuples, cf. plus bas) — l'ancien diff
        // relisait toutes les colonnes ré-extractibles du lot.
        let column_items: &[(i64, ExtractedFields)] =
            if column_fields.is_empty() { &[] } else { items };

        if !column_items.is_empty() {
            // FK jurisdiction_code : les lignes du référentiel doivent exister
            // avant l'UPDATE (référentiel ouvert, ADR 0146) ; la map résout le
            // code source vers le code canonique (ADR 0201).
            let jurisdiction_codes = if column_fields.contains(&"jurisdiction_code") {
                let juris: Vec<JurisdictionRow> = column_items
                    .iter()
                    .filter_map(|(_, e)| e.jurisdiction.clone())
                    .collect();
                self.ensure_jurisdictions(&juris).await?
            } else {
                Default::default()
            };
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
        if selected.contains(&PARTIES_FIELD) {
            let party_items: Vec<super::parties::DecisionPartyWriteItem> = items
                .iter()
                .filter(|(_, e)| overwrite || e.parties.is_some())
                .map(|(id, e)| (*id, e.parties.clone()))
                .collect();
            self.replace_decision_parties_bulk(&party_items).await?;
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
        let jurisdiction_code = match e.and_then(|e| e.jurisdiction.clone()) {
            Some(j) => self
                .ensure_jurisdictions(std::slice::from_ref(&j))
                .await?
                .remove(&j.source_code),
            None => None,
        };
        let row = self
            .conn
            .query_one(
                "
                INSERT INTO decisions (
                  jurisdiction_type, public_id, updated_at,
                  date_lecture, date_audience, docket_numbers,
                  publication_codes, extract_version,
                  full_text, embed_version, ecli, canonical_ref,
                  solution_uid, procedure_uid, office_uid, legal_domain_uid,
                  publication_uid, jurisdiction_code,
                  applicant_counsel_names, applicant_law_firms, applicant_companies,
                  defendant_counsel_names, defendant_law_firms, defendant_companies,
                  intervenors, themes, chamber_position, chamber_uid, formation_uid,
                  search_title
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                        $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30)
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
                    &decision.texte_integral_clean,
                    &embed_version,
                    &decision.ecli,
                    &canonical_ref,
                    &e.and_then(|e| e.solution_uid.clone()),
                    &e.and_then(|e| e.procedure_uid.clone()),
                    &e.and_then(|e| e.office_uid.clone()),
                    &e.and_then(|e| e.legal_domain_uid.clone()),
                    &e.and_then(|e| e.publication_uid.clone()),
                    &jurisdiction_code,
                    &e.map(|e| e.applicant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.applicant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.applicant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_counsel_names.clone())
                        .unwrap_or_default(),
                    &e.map(|e| e.defendant_law_firms.clone()).unwrap_or_default(),
                    &e.map(|e| e.defendant_companies.clone()).unwrap_or_default(),
                    &e.map(|e| e.intervenors.clone()).unwrap_or_default(),
                    &e.map(|e| e.themes.clone()).unwrap_or_default(),
                    &e.and_then(|e| e.chamber_position.clone()),
                    &e.and_then(|e| e.chamber_uid.clone()),
                    &e.and_then(|e| e.formation_uid.clone()),
                    &e.and_then(|e| e.search_title.clone()),
                ],
            )
            .await?;
        let decision_id: i64 = row.get(0);
        if let Some(e) = e {
            self.replace_citations(decision_id, e.citation_occurrences.as_deref())
                .await?;
            self.replace_decision_links(decision_id, e.decision_links.as_deref())
                .await?;
            self.replace_decision_parties(decision_id, e.parties.as_deref())
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

/// Code canonique d'une unité juridictionnelle (ADR 0201) : TJ/TCOM = slug de
/// la ville (`tj_paris`, `tcom_lyon`) ; tout autre type reprend le code source
/// tel quel (singletons `cc`/`ce`… et slugs `ca_`/`caa_`/`ta_` déjà en ville).
/// Sans ville exploitable, repli franc sur le code source.
fn canonical_jurisdiction_code(
    jurisdiction_type: &str,
    city: Option<&str>,
    source_code: &str,
) -> String {
    if !matches!(jurisdiction_type, "TJ" | "TCOM") {
        return source_code.to_string();
    }
    match city.map(slugify_city).filter(|s| !s.is_empty()) {
        Some(slug) => format!("{}_{slug}", jurisdiction_type.to_lowercase()),
        None => source_code.to_string(),
    }
}

/// Slug ASCII d'un nom de ville — miroir exact de l'expression SQL de la
/// migration 0137 (accents français pliés, toute séquence non alphanumérique
/// réduite à un `_`, bornes nettoyées).
fn slugify_city(city: &str) -> String {
    let mut out = String::with_capacity(city.len());
    for c in city.to_lowercase().chars() {
        let folded = match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'î' | 'ï' => 'i',
            'ó' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ÿ' => 'y',
            'ñ' => 'n',
            'œ' => {
                out.push('o');
                'e'
            }
            c => c,
        };
        if folded.is_ascii_alphanumeric() {
            out.push(folded);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_codes_slug_tj_tcom_cities() {
        // Spec ADR 0201, miroir de l'expression SQL de la migration 0137.
        for (city, expected) in [
            ("Paris", "tj_paris"),
            ("Saint-Étienne", "tj_saint_etienne"),
            ("Épinal", "tj_epinal"),
            ("Châlons-en-Champagne", "tj_chalons_en_champagne"),
            ("Les Sables-d'Olonne", "tj_les_sables_d_olonne"),
            ("Saint-Denis de la Réunion", "tj_saint_denis_de_la_reunion"),
            // ç → c : le translate SQL de la 0137 était désaligné (réparé en
            // 0138), le miroir Rust fait foi.
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
        // Sans ville : repli franc sur le code source.
        assert_eq!(
            canonical_jurisdiction_code("TJ", None, "tj75056"),
            "tj75056"
        );
        // Hors TJ/TCOM : le code source est déjà canonique.
        assert_eq!(
            canonical_jurisdiction_code("CA", Some("Paris"), "ca_paris"),
            "ca_paris"
        );
        assert_eq!(canonical_jurisdiction_code("CC", None, "cc"), "cc");
    }
}
