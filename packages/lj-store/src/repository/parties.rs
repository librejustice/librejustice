//! Acteurs par décision (ADR 0181/0182) : écriture au fil de l'eau par le
//! patron de `cases.rs` (garde jamais-dégrader, skip-diff, DELETE + COPY
//! binaire versionné), backfill de rattrapage par TRUNCATE + COPY,
//! résolution des clés pendantes vers le référentiel d'entités (ADR 0179).
//!
//! L'appelant (lj-ingest) plie et calcule `resolve_key`/`nature`/spans en
//! Rust (`lj_extract::parties`) — le SQL ne replie jamais.

use std::collections::{HashMap, HashSet};

use super::types::DecisionPartyRow;
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Item d'écriture bulk : `(decision_id, acteurs)`. `None` = extraction sans
/// couche parties (écrit un set vide).
pub type DecisionPartyWriteItem = (i64, Option<Vec<DecisionPartyRow>>);

/// Empreinte d'un acteur pour le skip-diff : tout sauf `entity_uid` (état de
/// résolution, pas une donnée d'extraction — un set inchangé garde ses
/// liens) et `ord` (régénéré). `resolve_key` et `barreau` en font partie :
/// depuis l'ADR 0188 la clé dépend du contexte documentaire, plus seulement
/// de la valeur. Spans en `Option` : le fonds backfillé d'avant-vague (NULL)
/// diffère d'un set calculé vide.
type PartyFingerprint = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<i32>>,
    Option<Vec<i32>>,
);

fn fingerprint(r: &DecisionPartyRow) -> PartyFingerprint {
    (
        r.quality.clone(),
        r.side.clone(),
        r.value.clone(),
        r.resolve_key.clone(),
        r.nature.clone(),
        r.barreau.clone(),
        r.role.clone(),
        Some(r.char_starts.clone()),
        Some(r.char_ends.clone()),
    )
}

const COPY_SQL: &str = "COPY decision_party (decision_id, ord, quality, side, value, \
     resolve_key, char_starts, char_ends, nature, barreau, role, extract_version) \
     FROM STDIN (FORMAT binary)";
const COPY_TYPES: &[Type] = &[
    Type::INT8,
    Type::INT4,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::INT4_ARRAY,
    Type::INT4_ARRAY,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::INT2,
];

impl DecisionRepository<'_> {
    /// Vide la table avant backfill intégral — idempotence par remplacement
    /// (règle #7, ADR 0181). À appeler dans la transaction du backfill.
    pub async fn decision_party_clear(&self) -> Result<()> {
        self.conn.execute("TRUNCATE decision_party", &[]).await?;
        Ok(())
    }

    /// COPY binaire d'un lot d'acteurs (backfill de rattrapage). `version` =
    /// `decisions.extract_version` de la décision source — les lignes
    /// reflètent les colonnes plates de cette version.
    pub async fn decision_party_copy(
        &self,
        items: &[(i64, i16, Vec<DecisionPartyRow>)],
    ) -> Result<()> {
        let sink = self.conn.copy_in(COPY_SQL).await?;
        let writer = BinaryCopyInWriter::new(sink, COPY_TYPES);
        tokio::pin!(writer);
        for (decision_id, version, rows) in items {
            for (ord, r) in rows.iter().enumerate() {
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &(ord as i32),
                        &r.quality,
                        &r.side,
                        &r.value,
                        &r.resolve_key,
                        &r.char_starts,
                        &r.char_ends,
                        &r.nature,
                        &r.barreau,
                        &r.role,
                        version,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        Ok(())
    }

    /// (Ré)écrit les acteurs d'UNE décision — sucre sur
    /// [`Self::replace_decision_parties_bulk`].
    pub async fn replace_decision_parties(
        &self,
        decision_id: i64,
        parties: Option<&[DecisionPartyRow]>,
    ) -> Result<()> {
        self.replace_decision_parties_bulk(&[(decision_id, parties.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les acteurs d'un lot de décisions (ADR 0182) : garde
    /// jamais-dégrader (version > `EXTRACT_VERSION` = révision manuelle /
    /// gold, jamais remplacée), skip-diff des sets inchangés (un set
    /// inchangé garde ses `entity_uid` résolus), puis DELETE + COPY binaire.
    /// Les lignes réécrites repartent pendantes — la résolution de fin de
    /// run les reprend. **La transaction appartient à l'appelant** (comme
    /// `cases.rs`). Idempotent.
    #[tracing::instrument(name = "db.replace_decision_parties_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_decision_parties_bulk(
        &self,
        items: &[DecisionPartyWriteItem],
    ) -> Result<()> {
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
                "SELECT decision_id, quality, side, value, resolve_key, nature, barreau, \
                        role, char_starts, char_ends \
                 FROM decision_party \
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<PartyFingerprint>> = HashMap::new();
        for row in &db_rows {
            current.entry(row.get(0)).or_default().insert((
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
                row.get(5),
                row.get(6),
                row.get(7),
                row.get(8),
                row.get(9),
            ));
        }
        let empty = HashSet::new();
        let changed: Vec<&DecisionPartyWriteItem> = items
            .iter()
            .filter(|(id, parties)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<PartyFingerprint> = parties
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(fingerprint)
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(());
        }

        // Asserts durs à l'entrée (règle #12) : tableaux parallèles, spans
        // triés strictement, non chevauchants, non vides.
        for (decision_id, parties) in &changed {
            for r in parties.as_deref().unwrap_or(&[]) {
                if r.char_starts.len() != r.char_ends.len() {
                    return Err(StoreError::Invalid(format!(
                        "decision_party invalide (décision {decision_id}) : tableaux de \
                         spans non parallèles ({} vs {})",
                        r.char_starts.len(),
                        r.char_ends.len()
                    )));
                }
                let mut prev_end = i32::MIN;
                for (s, e) in r.char_starts.iter().zip(&r.char_ends) {
                    if s >= e || *s < prev_end {
                        return Err(StoreError::Invalid(format!(
                            "decision_party invalide (décision {decision_id}) : span \
                             [{s}, {e}) après end={prev_end} — tri/chevauchement violé"
                        )));
                    }
                    prev_end = *e;
                }
            }
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM decision_party WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let sink = self.conn.copy_in(COPY_SQL).await?;
        let writer = BinaryCopyInWriter::new(sink, COPY_TYPES);
        tokio::pin!(writer);
        for (decision_id, parties) in &changed {
            for (ord, r) in parties.as_deref().unwrap_or(&[]).iter().enumerate() {
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &(ord as i32),
                        &r.quality,
                        &r.side,
                        &r.value,
                        &r.resolve_key,
                        &r.char_starts,
                        &r.char_ends,
                        &r.nature,
                        &r.barreau,
                        &r.role,
                        &lj_core::EXTRACT_VERSION,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        Ok(())
    }

    /// Résout les clés pendantes vers `entity` par dénomination pliée NON
    /// AMBIGUË (ADR 0181 V1) : une clé qui matche plusieurs entités
    /// distinctes reste pendante. Deux périmètres : morales (`party`,
    /// `law_firm`) contre `siren:`/`rna:` — complétées, pour les `law_firm`
    /// hors CC/CE, par l'égalité de forme normalisée restreinte à la
    /// catégorie `cabinets` (ADR 0243) —, et avocats (`counsel_name`)
    /// contre `cnb:` (ADR 0188 — clé nom complet ± rotations de tokens,
    /// homonymes départagés par le barreau du uid, décisions CC/CE exclues :
    /// avocats aux Conseils hors CNB), avec sous-étage patronyme composé
    /// unique national (ADR 0195, `entity.surname_key`) ; et — complément
    /// (ADR 0190) — les
    /// seules décisions CC/CE contre `oacc:` (`counsel_name` → avocats,
    /// `law_firm` → sociétés `oacc:firm-%` par sous-ensemble de tokens,
    /// ADR 0242), unicité du hit exigée, sans discriminant barreau
    /// (registre minuscule). `intervenor` est gaté (ADR 0182 §7).
    /// Renvoie le nombre de lignes liées.
    #[tracing::instrument(name = "db.resolve_pending_parties", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_parties(&self) -> Result<u64> {
        // Jointure clés distinctes × 32 M dénominations : dépasse le
        // `statement_timeout` du pool (30 s) — levé localement, transaction
        // dédiée (précédent `refresh_article_denorm`). Idempotent (#7).
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let n = self
                .conn
                .execute(
                    "WITH keys AS ( \
                     SELECT DISTINCT resolve_key FROM decision_party \
                     WHERE entity_uid IS NULL \
                       AND quality IN ('party', 'law_firm') \
                 ), names AS ( \
                     SELECT e.uid, f.folded \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                     WHERE e.uid NOT LIKE 'cnb:%' \
                       AND e.uid NOT LIKE 'oacc:%' \
                 ), hit AS ( \
                     SELECT n.folded, min(n.uid) AS uid \
                     FROM names n \
                     JOIN keys k ON k.resolve_key = n.folded \
                     GROUP BY n.folded \
                     HAVING count(DISTINCT n.uid) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = h.uid \
                 FROM hit h \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality IN ('party', 'law_firm') \
                   AND p.resolve_key = h.folded",
                    &[],
                )
                .await?;
            // Cabinets normalisés (ADR 0243) : les `law_firm` encore pendantes
            // hors CC/CE se comparent en forme normalisée (formes sociales,
            // génériques du métier, conjonctions et tokens < 2 chars jetés,
            // particules de patronymes conservées, ordre conservé) contre les
            // dénominations — courantes et historiques — des entités de
            // catégorie `cabinets` uniquement. Clés à `[` exclues
            // (placeholders d'anonymisation), unicité du hit exigée.
            // `lj_fold_all` peut émettre des folded dupliqués (périodes
            // SIRENE, variantes qui plient pareil) : dédup AVANT tokenisation,
            // sinon string_agg double les tokens (« actis actis ») — norme
            // corrompue, faux liens par disparition de l'homonyme.
            let g = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key AS k \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'law_firm' \
                       AND d.jurisdiction_type NOT IN ('CC', 'CE') \
                       AND strpos(p.resolve_key, '[') = 0 \
                 ), knorm AS ( \
                     SELECT k, string_agg(tok, ' ' ORDER BY ord) AS norm \
                     FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') \
                          WITH ORDINALITY AS t(tok, ord) \
                     WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                         'scp','sarl','sarlu','sas','sasu','selarl','selarlu', \
                         'selas','selasu','selafa','sel','aarpi','sep', \
                         'societe','societes','cabinet','cabinets','etude', \
                         'office','avocat','avocats','associe','associes', \
                         'conseil','conseils','et','ou') \
                     GROUP BY k \
                 ), dnorm AS MATERIALIZED ( \
                     SELECT dd.entity_uid, string_agg(tok, ' ' ORDER BY ord) AS norm \
                     FROM (SELECT DISTINCT e.uid AS entity_uid, f.folded \
                           FROM entity e, \
                                LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                           WHERE e.category = 'cabinets' AND e.uid LIKE 'siren:%') dd, \
                          LATERAL regexp_split_to_table(dd.folded, '[^[:alnum:]]+') \
                          WITH ORDINALITY AS t(tok, ord) \
                     WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                         'scp','sarl','sarlu','sas','sasu','selarl','selarlu', \
                         'selas','selasu','selafa','sel','aarpi','sep', \
                         'societe','societes','cabinet','cabinets','etude', \
                         'office','avocat','avocats','associe','associes', \
                         'conseil','conseils','et','ou') \
                     GROUP BY dd.entity_uid, dd.folded \
                 ), pick AS ( \
                     SELECT h.k, min(h.entity_uid) AS uid \
                     FROM (SELECT kn.k, dn.entity_uid \
                           FROM knorm kn JOIN dnorm dn ON dn.norm = kn.norm \
                           GROUP BY kn.k, dn.entity_uid) h \
                     GROUP BY h.k HAVING count(*) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'law_firm' \
                   AND p.resolve_key = pk.k \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type NOT IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            let c = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key, p.barreau \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'counsel_name' \
                       AND strpos(p.resolve_key, ' ') > 0 \
                       AND d.jurisdiction_type NOT IN ('CC', 'CE') \
                 ), variant AS ( \
                     SELECT resolve_key, barreau, resolve_key AS k FROM pend \
                     UNION \
                     SELECT resolve_key, barreau, \
                            regexp_replace(resolve_key, '^(\\S+) (.*)$', '\\2 \\1') FROM pend \
                     UNION \
                     SELECT resolve_key, barreau, \
                            regexp_replace(resolve_key, '^(.*) (\\S+)$', '\\2 \\1') FROM pend \
                 ), cnb_names AS ( \
                     SELECT e.uid AS entity_uid, f.folded \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                     WHERE e.uid LIKE 'cnb:%' \
                 ), cand AS ( \
                     SELECT v.resolve_key, v.barreau, n.entity_uid \
                     FROM variant v \
                     JOIN cnb_names n ON n.folded = v.k \
                 ), pick AS ( \
                     SELECT resolve_key, barreau, \
                            CASE \
                              WHEN count(DISTINCT entity_uid) = 1 THEN min(entity_uid) \
                              WHEN count(DISTINCT entity_uid) FILTER \
                                     (WHERE split_part(entity_uid, ':', 2) = barreau) = 1 \
                                THEN min(entity_uid) FILTER \
                                     (WHERE split_part(entity_uid, ':', 2) = barreau) \
                            END AS uid \
                     FROM cand GROUP BY resolve_key, barreau \
                 ) \
                 UPDATE decision_party p SET entity_uid = k.uid \
                 FROM pick k, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'counsel_name' \
                   AND p.resolve_key = k.resolve_key \
                   AND p.barreau IS NOT DISTINCT FROM k.barreau \
                   AND k.uid IS NOT NULL \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type NOT IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            // Sous-étage patronyme composé (ADR 0195) : une clé counsel encore
            // pendante, multi-token une fois les tirets normalisés en espaces,
            // qui ne matche AUCUNE dénomination complète cnb (as-is ni
            // rotations — garde anti-collision prénom+nom ↔ patronyme
            // composé homonyme) et dont le patronyme est UNIQUE au registre
            // (`entity.surname_key`, émis par le chargeur CNB) → lie. Hors
            // CC/CE comme l'étage nom-complet.
            let s = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'counsel_name' \
                       AND (strpos(p.resolve_key, ' ') > 0 \
                            OR strpos(p.resolve_key, '-') > 0) \
                       AND d.jurisdiction_type NOT IN ('CC', 'CE') \
                 ), cnb_names AS ( \
                     SELECT f.folded \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                     WHERE e.uid LIKE 'cnb:%' \
                 ), eligible AS ( \
                     SELECT k.resolve_key FROM pend k \
                     WHERE NOT EXISTS ( \
                         SELECT 1 FROM cnb_names n \
                         WHERE n.folded IN ( \
                               k.resolve_key, \
                               regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                               regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1'))) \
                 ), pick AS ( \
                     SELECT g.resolve_key, min(e.uid) AS uid \
                     FROM eligible g \
                     JOIN entity e ON e.surname_key = translate(g.resolve_key, '-', ' ') \
                     WHERE e.uid LIKE 'cnb:%' \
                     GROUP BY g.resolve_key HAVING count(DISTINCT e.uid) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'counsel_name' \
                   AND p.resolve_key = pk.resolve_key \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type NOT IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            // Avocats aux Conseils (ADR 0190) : complément de l'exclusion
            // CC/CE de l'ADR 0188 — le registre `oacc:` ne résout QUE les
            // décisions `jurisdiction_type IN ('CC', 'CE')`. Registre minuscule
            // (~140), pas de discriminant barreau : on exige l'unicité du hit
            // dans `oacc:`. Deux volets : `counsel_name` → avocats (nom complet
            // ± rotations de tokens, ou nom-seul == patronyme d'un avocat) ;
            // `law_firm` → sociétés (`oacc:firm-%`) par sous-ensemble de
            // tokens (ADR 0242).
            let o = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key AS k \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'counsel_name' \
                       AND d.jurisdiction_type IN ('CC', 'CE') \
                 ), variant AS ( \
                     SELECT k, k AS v FROM pend \
                     UNION \
                     SELECT k, regexp_replace(k, '^(\\S+) (.*)$', '\\2 \\1') \
                       FROM pend WHERE strpos(k, ' ') > 0 \
                     UNION \
                     SELECT k, regexp_replace(k, '^(.*) (\\S+)$', '\\2 \\1') \
                       FROM pend WHERE strpos(k, ' ') > 0 \
                 ), av AS ( \
                     SELECT e.uid AS entity_uid, f.folded, \
                            split_part(f.folded, ' ', -1) AS surname \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                     WHERE e.uid LIKE 'oacc:%' AND e.uid NOT LIKE 'oacc:firm-%' \
                 ), cand AS ( \
                     SELECT v.k, a.entity_uid FROM variant v JOIN av a ON a.folded = v.v \
                     UNION \
                     SELECT p.k, a.entity_uid FROM pend p JOIN av a ON a.surname = p.k \
                       WHERE strpos(p.k, ' ') = 0 \
                 ), pick AS ( \
                     SELECT k, min(entity_uid) AS uid \
                     FROM cand GROUP BY k HAVING count(DISTINCT entity_uid) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'counsel_name' \
                   AND p.resolve_key = pk.k \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            // Sous-ensemble de tokens (ADR 0242) : la clé pliée est
            // tokenisée sur les non-alphanumériques, débarrassée des mots de
            // forme sociale / génériques et des tokens < 2 chars (initiales,
            // placeholders anonymisés) ; elle lie si tous ses tokens
            // restants figurent dans la dénomination d'exactement un
            // cabinet. Généralise l'égalité exacte de l'ADR 0190 initial.
            let f = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key AS k \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'law_firm' \
                       AND d.jurisdiction_type IN ('CC', 'CE') \
                 ), ptok AS ( \
                     SELECT k, t.tok \
                     FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') AS t(tok) \
                     WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                         'scp','sarl','sarlu','sas','selas','selarl','selafa', \
                         'selasu','aarpi','societe','cabinet','avocat','avocats', \
                         'associe','associes','et','ou','de','du','des','la','le', \
                         'les','au','aux') \
                     GROUP BY k, t.tok \
                 ), nkey AS ( \
                     SELECT k, count(*) AS nt FROM ptok GROUP BY k \
                 ), ftok AS MATERIALIZED ( \
                     SELECT e.uid AS entity_uid, t.tok \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded), \
                          LATERAL regexp_split_to_table(f.folded, '[^[:alnum:]]+') AS t(tok) \
                     WHERE e.uid LIKE 'oacc:firm-%' AND t.tok <> '' \
                     GROUP BY e.uid, t.tok \
                 ), hit AS ( \
                     SELECT pt.k, f.entity_uid \
                     FROM ptok pt \
                     JOIN ftok f ON f.tok = pt.tok \
                     JOIN nkey nk ON nk.k = pt.k \
                     GROUP BY pt.k, f.entity_uid, nk.nt \
                     HAVING count(*) = nk.nt \
                 ), pick AS ( \
                     SELECT k, min(entity_uid) AS uid \
                     FROM hit GROUP BY k HAVING count(DISTINCT entity_uid) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'law_firm' \
                   AND p.resolve_key = pk.k \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            Ok(n + g + c + s + o + f)
        }
        .await;
        match updated {
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

    /// Volumétrie de liaison : (total, résolues, dont `siren:`, dont `rna:`,
    /// dont `cnb:`, dont `oacc:`).
    pub async fn decision_party_stats(&self) -> Result<(i64, i64, i64, i64, i64, i64)> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*), count(entity_uid), \
                        count(*) FILTER (WHERE entity_uid LIKE 'siren:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'rna:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'cnb:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'oacc:%') \
                 FROM decision_party",
                &[],
            )
            .await?;
        Ok((
            row.get(0),
            row.get(1),
            row.get(2),
            row.get(3),
            row.get(4),
            row.get(5),
        ))
    }
}
