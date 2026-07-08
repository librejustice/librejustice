//! Citations de jurisprudence (ADR 0165) : une occurrence = une ligne
//! `case_citation` `(decision_id, char_start, char_end, target_ref,
//! target_decision_id, extract_version)`. `target_ref` = clé pendante par
//! famille ; `target_decision_id` posé par la résolution SQL (à l'écriture
//! puis relink post-ingest — une cible peut arriver après ses citations).
//!
//! Écriture par le patron de `citations.rs` : garde jamais-dégrader,
//! skip-diff des sets inchangés (préserve les cibles déjà résolues),
//! DELETE + COPY binaire.

use std::collections::{HashMap, HashSet};

use super::types::CaseCitationRow;
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Item d'écriture bulk : `(decision_id, citations)`. `None` = extraction
/// sans couche jurisprudence (écrit un set vide).
pub type CaseCitationWriteItem = (i64, Option<Vec<CaseCitationRow>>);

/// Empreinte d'une occurrence pour le skip-diff : `(char_start, char_end,
/// target_ref)` — `target_decision_id` est un état de résolution, pas une
/// donnée d'extraction, il ne re-déclenche pas d'écriture.
type CaseFingerprint = (i32, i32, String);

impl DecisionRepository<'_> {
    /// (Ré)écrit les citations de jurisprudence d'UNE décision — sucre sur
    /// [`Self::replace_case_citations_bulk`].
    pub async fn replace_case_citations(
        &self,
        decision_id: i64,
        citations: Option<&[CaseCitationRow]>,
    ) -> Result<()> {
        self.replace_case_citations_bulk(&[(decision_id, citations.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les citations de jurisprudence d'un lot de décisions (ADR 0165) :
    /// garde jamais-dégrader (version > `EXTRACT_VERSION` = révision manuelle,
    /// jamais remplacée), skip-diff des sets `(char_start, char_end,
    /// target_ref)` inchangés (la passe intégrale hebdo ne doit pas churner,
    /// et un set inchangé garde ses cibles résolues), puis DELETE + COPY
    /// binaire. Les lignes réécrites repartent pendantes
    /// (`target_decision_id` NULL) — la résolution par famille les reprend.
    /// **La transaction appartient à l'appelant** (comme `citations.rs`).
    /// Idempotent.
    #[tracing::instrument(name = "db.replace_case_citations_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_case_citations_bulk(&self, items: &[CaseCitationWriteItem]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let protected: HashSet<i64> = self
            .conn
            .query(
                "SELECT id FROM decisions WHERE id = ANY($1) AND extract_version > $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();

        let db_rows = self
            .conn
            .query(
                "SELECT decision_id, char_start, char_end, target_ref FROM case_citation
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<CaseFingerprint>> = HashMap::new();
        for row in &db_rows {
            current
                .entry(row.get(0))
                .or_default()
                .insert((row.get(1), row.get(2), row.get(3)));
        }
        let empty = HashSet::new();
        let changed: Vec<&CaseCitationWriteItem> = items
            .iter()
            .filter(|(id, citations)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<CaseFingerprint> = citations
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|c| (c.char_start, c.char_end, c.target_ref.clone()))
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(());
        }

        // Asserts durs à l'entrée (tri strict, zéro chevauchement, span non
        // vide) — l'amont les garantit, une violation est un bug (règle #12).
        for (decision_id, citations) in &changed {
            let mut prev_end = i32::MIN;
            for c in citations.as_deref().unwrap_or(&[]) {
                if c.char_start >= c.char_end || c.char_start < prev_end {
                    return Err(StoreError::Invalid(format!(
                        "case_citations invalides (décision {decision_id}) : span \
                         [{}, {}) après end={prev_end} — tri/chevauchement violé",
                        c.char_start, c.char_end
                    )));
                }
                prev_end = c.char_end;
            }
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM case_citation WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let has_rows = changed
            .iter()
            .any(|(_, c)| !c.as_deref().unwrap_or(&[]).is_empty());
        let sink = self
            .conn
            .copy_in(
                "COPY case_citation (decision_id, char_start, char_end, target_ref, \
                 extract_version) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[Type::INT8, Type::INT4, Type::INT4, Type::TEXT, Type::INT2],
        );
        tokio::pin!(writer);
        for (decision_id, citations) in &changed {
            for c in citations.as_deref().unwrap_or(&[]) {
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &c.char_start,
                        &c.char_end,
                        &c.target_ref,
                        &lj_core::EXTRACT_VERSION,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        if has_rows {
            self.resolve_case_citations_scoped(Some(&changed_ids))
                .await?;
        }
        Ok(())
    }

    /// Résout en batch toutes les citations pendantes dont la cible est en
    /// base (relink post-ingest / passe hebdo — une cible peut arriver après
    /// ses citations). Renvoie le nombre de citations résolues.
    #[tracing::instrument(name = "db.resolve_pending_case_citations", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_case_citations(&self) -> Result<u64> {
        self.resolve_case_citations_scoped(None).await
    }

    /// Résolution par famille (ADR 0165 §4) contre les `docket_numbers` des
    /// cibles (GIN, migration 0113), clé reformatée côté requête au format
    /// stocké. Match unique (`HAVING`-équivalent) → `target_decision_id` ;
    /// CJUE multi-match → snap au document ARRÊT (type CELEX `J` du
    /// `source_uid`, jamais les conclusions AG) ; ambigu ou introuvable →
    /// reste pendant, relinkable. `ids` = portée at-write (lignes fraîchement
    /// écrites), `None` = tout le pendant.
    async fn resolve_case_citations_scoped(&self, ids: Option<&[i64]>) -> Result<u64> {
        let mut resolved = 0u64;
        let mut sqls = resolve_case_family_sqls(ids.is_some());
        sqls.push(resolve_from_links_sql(ids.is_some()));
        for sql in sqls {
            let n = match ids {
                Some(ids) => self.conn.execute(sql.as_str(), &[&ids]).await?,
                None => self.conn.execute(sql.as_str(), &[]).await?,
            };
            resolved += n;
        }
        Ok(resolved)
    }
}

/// Pont chronologie → citations (spans pontés métadonnée, v22) : une citation
/// pendante dont la clé est CELLE d'un lien de chronologie résolu de la même
/// décision hérite de sa cible. Les clés `contested`
/// (`ca|<loc>|<rg>|<date>`, `tj|…`, `tcom|…`) n'ont pas de famille docket —
/// seul ce pont les résout. Les liens sont écrits et résolus AVANT les
/// citations dans le flux d'écriture (`decisions.rs`).
fn resolve_from_links_sql(scoped: bool) -> String {
    let scope = if scoped {
        " AND c.decision_id = ANY($1)"
    } else {
        ""
    };
    format!(
        "UPDATE case_citation c \
         SET target_decision_id = dl.target_decision_id \
         FROM decision_links dl \
         WHERE dl.decision_id = c.decision_id \
           AND dl.target_ref = c.target_ref \
           AND dl.target_decision_id IS NOT NULL \
           AND c.target_decision_id IS NULL{scope}"
    )
}

/// Les sept UPDATE de résolution par famille — fonction pure (dumpable pour
/// validation psql). `scoped` borne le pendant aux décisions d'un lot (`$1`).
fn resolve_case_family_sqls(scoped: bool) -> Vec<String> {
    let scope = if scoped {
        " AND p.decision_id = ANY($1)"
    } else {
        ""
    };
    [
            // CC : clé chiffres seuls (7) reformatée « 18-23.954 ».
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, \
                        substring(split_part(p.target_ref, '|', 2) from 1 for 2) || '-' || \
                        substring(split_part(p.target_ref, '|', 2) from 3 for 2) || '.' || \
                        substring(split_part(p.target_ref, '|', 2) from 5 for 3) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'cc|%' \
                   AND length(split_part(p.target_ref, '|', 2)) = 7 \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CC"),
            ),
            // CE / CEDH / CONSTIT : le numéro cité est le docket stocké.
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, split_part(p.target_ref, '|', 2) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'ce|%' \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CE"),
            ),
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, split_part(p.target_ref, '|', 2) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'cedh|%' \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CEDH"),
            ),
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, split_part(p.target_ref, '|', 2) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'constit|%' \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CONSTIT"),
            ),
            // RG (fond judiciaire) / AF (fond administratif TA/CAA, ADR 0165
            // [af]) : même mécanique, scopée au tribunal (jurisdiction_code de
            // la clé). Les clés NUES `rg||NUM` (juridiction non mappée) ne
            // résolvent JAMAIS par construction — exclues d'emblée, sinon chaque
            // numéro sonde le GIN pour rien (un RG court matche des dizaines de
            // tribunaux).
            resolve_by_jurisdiction_code("rg"),
            resolve_by_jurisdiction_code("af"),
            // CJUE : docket = l'AFFAIRE (arrêt + conclusions + ordonnances le
            // partagent) — match unique sinon snap au document arrêt (CELEX
            // `6YYYY<cour>J…` du source_uid). Clé nue (« aff. 6/64 ») → C-.
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, \
                        CASE WHEN split_part(p.target_ref, '|', 2) LIKE '_-%' \
                             THEN upper(split_part(p.target_ref, '|', 2)) \
                             ELSE 'C-' || split_part(p.target_ref, '|', 2) END AS docket, \
                        CASE WHEN split_part(p.target_ref, '|', 2) LIKE '_-%' \
                             THEN upper(left(split_part(p.target_ref, '|', 2), 1)) \
                             ELSE 'C' END AS court \
                 FROM pending p WHERE p.target_ref LIKE 'cjue|%' \
                 ), resolved AS ( \
                   SELECT r.target_ref, \
                          (SELECT CASE \
                                    WHEN count(*) = 1 THEN min(c.id) \
                                    WHEN count(*) FILTER (WHERE c.is_arret) = 1 \
                                         THEN min(c.id) FILTER (WHERE c.is_arret) \
                                  END \
                           FROM (SELECT d.id, d.juridiction_type, EXISTS ( \
                                     SELECT 1 FROM decision_sources s \
                                     WHERE s.decision_id = d.id \
                                       AND s.source_uid LIKE 'cjue/6____' || r.court || 'J%') AS is_arret \
                                 FROM decisions d \
                                 WHERE d.deleted_at IS NULL \
                                   AND d.docket_numbers @> ARRAY[r.docket] \
                                 OFFSET 0) c \
                           WHERE c.juridiction_type = 'CJUE' \
                          ) AS tid \
                   FROM keys r){RESOLVE_TAIL}"
            ),
    ]
    .into_iter()
    .map(|family_sql| family_sql.replace("{SCOPE}", scope))
    .collect()
}

/// Tête commune des UPDATE de résolution : le pendant (scopé ou non) dédupliqué
/// par clé — une clé se résout une fois, quel que soit son nombre d'occurrences.
const RESOLVE_HEAD: &str = "WITH pending AS ( \
    SELECT DISTINCT p.target_ref FROM case_citation p \
    WHERE p.target_decision_id IS NULL{SCOPE} \
 ), keys AS (";

/// Queue commune : pose `target_decision_id` sur toutes les occurrences
/// pendantes de la clé résolue.
const RESOLVE_TAIL: &str = " \
 UPDATE case_citation c \
 SET target_decision_id = m.tid \
 FROM resolved m \
 WHERE c.target_ref = m.target_ref AND c.target_decision_id IS NULL \
   AND m.tid IS NOT NULL";

/// Corps « match unique par type de juridiction » : ferme le CTE `keys` et
/// résout chaque clé contre les décisions du type portant le docket.
///
/// Le lookup docket est clôturé (`OFFSET 0`) pour forcer le GIN seul : sans la
/// clôture, le planner combine en `BitmapAnd` avec l'index `juridiction_type`
/// et scanne des centaines de milliers d'entrées PAR clé (mesuré : 8,5 s par
/// batch at-write sur la famille CC contre 1 ligne attendue du GIN). Le filtre
/// juridiction s'applique après, sur les ~1-3 candidats du docket.
/// Corps « match unique par `jurisdiction_code` » : famille du fond (`rg`
/// judiciaire, `af` administratif) — la clé porte `{prefix}|{code}|{docket}`,
/// on résout le docket contre les décisions du MÊME `jurisdiction_code`. Clés
/// nues (`{prefix}||NUM`, code vide) exclues d'emblée (jamais résolubles). Même
/// clôture `OFFSET 0` que [`resolve_unique_by_type`] pour forcer le GIN seul.
fn resolve_by_jurisdiction_code(prefix: &str) -> String {
    format!(
        "{RESOLVE_HEAD} \
         SELECT p.target_ref, split_part(p.target_ref, '|', 3) AS docket, \
                split_part(p.target_ref, '|', 2) AS code \
         FROM pending p WHERE p.target_ref LIKE '{prefix}|%' \
           AND split_part(p.target_ref, '|', 2) <> '' \
         ), resolved AS ( \
           SELECT r.target_ref, \
                  (SELECT min(x.id) FROM \
                     (SELECT d.id, d.jurisdiction_code FROM decisions d \
                      WHERE d.deleted_at IS NULL \
                        AND d.docket_numbers @> ARRAY[r.docket] \
                      OFFSET 0) x \
                   WHERE x.jurisdiction_code = r.code \
                   HAVING count(*) = 1) AS tid \
           FROM keys r){RESOLVE_TAIL}"
    )
}

fn resolve_unique_by_type(juridiction_type: &str) -> String {
    format!(
        "), resolved AS ( \
           SELECT r.target_ref, \
                  (SELECT min(x.id) FROM \
                     (SELECT d.id, d.juridiction_type FROM decisions d \
                      WHERE d.deleted_at IS NULL \
                        AND d.docket_numbers @> ARRAY[r.docket] \
                      OFFSET 0) x \
                   WHERE x.juridiction_type = '{juridiction_type}' \
                   HAVING count(*) = 1) AS tid \
           FROM keys r){RESOLVE_TAIL}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p lj-store resolve_case_family -- --nocapture` dumpe les
    /// sept UPDATE pour validation psql (table temporaire + clés sondes).
    #[test]
    fn resolve_case_family_sqls_cover_families_and_scope() {
        let all = resolve_case_family_sqls(false);
        assert_eq!(all.len(), 7);
        for fam in [
            "'cc|%'",
            "'ce|%'",
            "'cedh|%'",
            "'constit|%'",
            "'rg|%'",
            "'af|%'",
            "'cjue|%'",
        ] {
            assert!(all.iter().any(|s| s.contains(fam)), "famille {fam} absente");
        }
        for s in &all {
            assert!(s.contains("UPDATE case_citation"), "pas un UPDATE : {s}");
            assert!(!s.contains("$1"), "portée $1 en mode global : {s}");
            println!("{s};\n");
        }
        let scoped = resolve_case_family_sqls(true);
        assert!(scoped.iter().all(|s| s.contains("ANY($1)")));
    }
}
