//! Citations à plat (ADR 0145) : une occurrence = une ligne `legal_citation`
//! `(decision_id, char_start, char_end, ref_text_uid, ref_num_key,
//! extract_version)`, écrite déjà liée par le linker in-pass de l'ingest.
//! `ref_text_uid IS NULL` = non lié ; aucun état de résolution ni override en
//! base — une correction de masse est un commit dans le code/TSV du linker,
//! rejoué par la passe intégrale hebdomadaire.
//!
//! Écriture par décision / en bulk (skip-diff des sets inchangés), recompute
//! des arrays dénormalisés de facettes (migration 0098), et lectures du
//! `LinkSnapshot` (catalogue textes + articles).

use std::collections::{HashMap, HashSet};

use super::types::CitationOccurrenceRow;
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Item d'écriture bulk : `(decision_id, occurrences)`. `None` = extraction
/// sans couche citations (écrit un set vide).
pub type CitationWriteItem = (i64, Option<Vec<CitationOccurrenceRow>>);

/// Arrays de facettes d'une décision depuis ses occurrences (migration 0098) :
/// `legal_instruments` = `ref_text_uid` distincts, `legal_article_composite` =
/// `uid|num_key` distincts — `None` (SQL NULL) si vide, comme l'`ARRAY_AGG …
/// FILTER` des fonctions de resync. Tri bytewise = `COLLATE "C"` (0109).
fn citation_facet_arrays(
    rows: &[CitationOccurrenceRow],
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let mut instruments: Vec<String> = rows.iter().filter_map(|o| o.ref_text_uid.clone()).collect();
    instruments.sort_unstable();
    instruments.dedup();
    let mut composite: Vec<String> = rows
        .iter()
        .filter_map(|o| {
            let uid = o.ref_text_uid.as_deref()?;
            let num = o.ref_num_key.as_deref()?;
            Some(format!("{uid}|{num}"))
        })
        .collect();
    composite.sort_unstable();
    composite.dedup();
    (
        Some(instruments).filter(|v| !v.is_empty()),
        Some(composite).filter(|v| !v.is_empty()),
    )
}
/// Empreinte d'une occurrence pour le skip-diff : `(char_start, char_end,
/// ref_text_uid, ref_num_key)` (ADR 0145 — la table à plat porte la cible).
type OccurrenceFingerprint = (i32, i32, Option<String>, Option<String>);

impl DecisionRepository<'_> {
    /// (Ré)écrit la couche citations d'UNE décision — sucre sur
    /// [`Self::replace_citations_bulk`], l'unique chemin d'écriture recognizer
    /// (garde jamais-dégrader, skip-diff, séquence write → sync).
    pub async fn replace_citations(
        &self,
        decision_id: i64,
        occurrences: Option<&[CitationOccurrenceRow]>,
    ) -> Result<()> {
        self.replace_citations_bulk(&[(decision_id, occurrences.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit la couche citations d'un lot de décisions (ADR 0145) : garde
    /// jamais-dégrader (une décision à version > `EXTRACT_VERSION` — révision
    /// manuelle — n'est jamais remplacée par le recognizer), skip
    /// côté Rust des décisions au set d'occurrences inchangé, puis DELETE +
    /// COPY dans `legal_citation` et recompute des arrays de facettes (dans
    /// cet ordre — les arrays dérivent de `legal_citation`, migration 0098).
    /// Idempotent.
    #[tracing::instrument(name = "db.replace_citations_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_citations_bulk(&self, items: &[CitationWriteItem]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        // Invariant : jamais remplacer la couche citations d'une décision à
        // version > EXTRACT_VERSION (révision manuelle, ADR 0140/0141).
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
        let mut items = self.filter_unchanged_citations(items).await?;
        items.retain(|(id, _)| !protected.contains(id));
        if items.is_empty() {
            return Ok(());
        }
        let v9_items: Vec<(i64, &[CitationOccurrenceRow])> = items
            .iter()
            .map(|(id, occ)| (*id, occ.as_deref().unwrap_or(&[])))
            .collect();
        self.write_citation_occurrences(&v9_items, lj_core::EXTRACT_VERSION)
            .await
    }

    /// Sous-ensemble des items à réécrire (skip-diff du re-extract). Une décision
    /// est « changée » si son set d'occurrences `(char_start, char_end,
    /// ref_text_uid, ref_num_key)` diffère des lignes `legal_citation` à
    /// version ≤ courante de la DB — spans EUX-MÊMES comparés, pas seulement
    /// les cibles : une
    /// refonte du recognizer qui redistribue les offsets sans changer les
    /// cibles serait skippée à tort. Converge : après réécriture, l'empreinte
    /// DB == l'empreinte extraite → ne re-déclenche pas.
    pub(super) async fn filter_unchanged_citations(
        &self,
        items: &[CitationWriteItem],
    ) -> Result<Vec<CitationWriteItem>> {
        let decision_ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let db_rows = self
            .conn
            .query(
                "
                SELECT decision_id, char_start, char_end, ref_text_uid, ref_num_key
                FROM legal_citation
                WHERE decision_id = ANY($1) AND extract_version <= $2
                ",
                &[&decision_ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<OccurrenceFingerprint>> = HashMap::new();
        for row in &db_rows {
            current.entry(row.get(0)).or_default().insert((
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
            ));
        }
        let empty = HashSet::new();
        let mut changed = Vec::new();
        for (decision_id, occurrences) in items {
            let new_fingerprint: HashSet<OccurrenceFingerprint> = occurrences
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|o| {
                    (
                        o.char_start,
                        o.char_end,
                        o.ref_text_uid.clone(),
                        o.ref_num_key.clone(),
                    )
                })
                .collect();
            if &new_fingerprint != current.get(decision_id).unwrap_or(&empty) {
                changed.push((*decision_id, occurrences.clone()));
            }
        }
        Ok(changed)
    }

    /// Recalcule les arrays de facettes (`legal_instruments`,
    /// `legal_article_composite` sur `decisions` et `decision_chunks`) des
    /// décisions passées, depuis leurs arêtes fraîchement réécrites. Remplace
    /// les triggers 0076 (droppés en 0096, alternative re-confrontée et
    /// écartée par l'ADR 0147) : appelé par [`Self::write_citation_occurrences`]
    /// dans le même flux d'écriture. Les agrégats DISTINCT se calculent en
    /// Rust depuis les occurrences déjà en mémoire (tri bytewise = COLLATE
    /// "C", même ordre que les fonctions SQL de resync, migration 0109) — la
    /// version SQL relisait `legal_citation` à peine écrite (~250 ms/lot de
    /// 256 au profil reextract). Les UPDATE gardent `IS DISTINCT FROM` (zéro
    /// churn si rien ne change — l'index vectoriel des chunks coûte cher).
    async fn sync_citation_facet_arrays(
        &self,
        items: &[(i64, &[CitationOccurrenceRow])],
    ) -> Result<()> {
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> =
            Vec::with_capacity(items.len() * 3);
        let mut value_rows: Vec<String> = Vec::with_capacity(items.len());
        for (pos, (decision_id, rows)) in items.iter().enumerate() {
            let (instruments, composite) = citation_facet_arrays(rows);
            let base = pos * 3;
            if pos == 0 {
                value_rows.push(format!(
                    "((${})::bigint, (${})::text[], (${})::text[])",
                    base + 1,
                    base + 2,
                    base + 3
                ));
            } else {
                value_rows.push(format!("(${}, ${}, ${})", base + 1, base + 2, base + 3));
            }
            params.push(Box::new(*decision_id));
            params.push(Box::new(instruments));
            params.push(Box::new(composite));
        }
        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            params.iter().map(|p| p.as_ref()).collect();
        let values = value_rows.join(",");
        self.conn
            .execute(
                format!(
                    "UPDATE decisions d \
                     SET legal_instruments = src.instruments, \
                         legal_article_composite = src.composite \
                     FROM (VALUES {values}) AS src(id, instruments, composite) \
                     WHERE d.id = src.id \
                       AND (d.legal_instruments IS DISTINCT FROM src.instruments \
                         OR d.legal_article_composite IS DISTINCT FROM src.composite)"
                )
                .as_str(),
                &refs,
            )
            .await?;
        self.conn
            .execute(
                format!(
                    "UPDATE decision_chunks c \
                     SET legal_instruments = src.instruments, \
                         legal_article_composite = src.composite \
                     FROM (VALUES {values}) AS src(id, instruments, composite) \
                     WHERE c.decision_id = src.id \
                       AND (c.legal_instruments IS DISTINCT FROM src.instruments \
                         OR c.legal_article_composite IS DISTINCT FROM src.composite)"
                )
                .as_str(),
                &refs,
            )
            .await?;
        Ok(())
    }

    /// Écrit la couche citations d'un lot de décisions (ADR 0145) : DELETE
    /// des lignes existantes, **COPY binaire** dans `legal_citation`, puis
    /// recompute des arrays de facettes dérivés — la paire write → sync est
    /// scellée ICI et nulle part ailleurs (ADR 0147 : elle a cassé deux fois
    /// quand elle était répétée chez les appelants). **La transaction
    /// appartient à l'appelant** (mod.rs : le pipeline enveloppe ses
    /// batches) — aucun BEGIN/COMMIT ici : un COMMIT imbriqué commiterait la
    /// transaction externe prématurément et désatomiserait write ↔ sync.
    /// `extract_version` = `EXTRACT_VERSION` (recognizer, jamais appelé sur
    /// une décision à version supérieure — garde décision-niveau amont) ou
    /// toute version explicite (`replace_citations_at_version`, remplace
    /// tout). Asserts durs à
    /// l'entrée (tri strict par `char_start`, zéro chevauchement, `char_end >
    /// char_start`, `ref_num_key ⇒ ref_text_uid`) — l'amont les garantit, une
    /// violation ici est un bug (règle #12, pas de rattrapage silencieux).
    pub(super) async fn write_citation_occurrences(
        &self,
        items: &[(i64, &[CitationOccurrenceRow])],
        extract_version: i16,
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        for (decision_id, rows) in items {
            let mut prev_end = i32::MIN;
            for o in rows.iter() {
                if o.char_start >= o.char_end || o.char_start < prev_end {
                    return Err(StoreError::Invalid(format!(
                        "occurrences v9 invalides (décision {decision_id}) : span \
                         [{}, {}) après end={prev_end} — tri/chevauchement violé",
                        o.char_start, o.char_end
                    )));
                }
                if o.ref_num_key.is_some() && o.ref_text_uid.is_none() {
                    return Err(StoreError::Invalid(format!(
                        "occurrences v9 invalides (décision {decision_id}) : \
                         ref_num_key sans ref_text_uid à char_start={}",
                        o.char_start
                    )));
                }
                prev_end = o.char_end;
            }
        }

        let decision_ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM legal_citation WHERE decision_id = ANY($1)",
                &[&decision_ids],
            )
            .await?;
        let sink = self
            .conn
            .copy_in(
                "COPY legal_citation (decision_id, char_start, char_end, ref_text_uid, \
                 ref_num_key, extract_version) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT4,
                Type::INT4,
                Type::TEXT,
                Type::TEXT,
                Type::INT2,
            ],
        );
        tokio::pin!(writer);
        for (decision_id, rows) in items {
            for o in rows.iter() {
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &o.char_start,
                        &o.char_end,
                        &o.ref_text_uid,
                        &o.ref_num_key,
                        &extract_version,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        self.sync_citation_facet_arrays(items).await?;
        Ok(())
    }

    /// Slashnums UE (année/séquence) **cités mais absents** du catalogue, pour piloter
    /// l'ingestion EUR-Lex (ADR 0138). Renvoie `(nature, slashnum, edges)` où `nature ∈
    /// {reglement, directive}`, trié par mentions décroissantes (une ligne
    /// `legal_citation` non liée = une mention). « Absent » = aucun `legal_text` de
    /// la nature correspondante dont le `title` porte ce slashnum. Lecture seule.
    #[tracing::instrument(name = "db.cited_eu_slashnums_missing", skip(self), fields(db.system = "postgresql"))]
    pub async fn cited_eu_slashnums_missing(&self) -> Result<Vec<(String, String, i64)>> {
        // ADR 0145 : les clés de capture ne sont plus stockées — la forme citée se
        // retranche de `full_text` par les spans **non liés** de `legal_citation`
        // (offsets codepoints 0143 = sémantique caractère de `substring`). Scan
        // lourd (détoaste chaque décision porteuse) : commande manuelle rare.
        let rows = self
            .conn
            .query(
                r#"
                WITH cited AS (
                    SELECT
                        CASE WHEN raw ~* '^\s*(?:le |la |les |l['''']? ?|du |de la |de l['''']? ?)?\s*r[eè]glement' THEN 'reglement'
                             WHEN raw ~* '^\s*(?:le |la |les |l['''']? ?|du |de la |de l['''']? ?)?\s*directive' THEN 'directive' END AS nature,
                        (regexp_match(raw,
                            '(?i)^\s*(?:le |la |les |l['''']? ?|du |de la |de l['''']? ?)?\s*(?:directive|r[eè]glement)[^0-9]{0,40}(\d{1,4}/\d{1,4})'))[1] AS slashnum
                    FROM (
                        SELECT substring(d.full_text FROM lc.char_start + 1 FOR lc.char_end - lc.char_start) AS raw
                        FROM legal_citation lc
                        JOIN decisions d ON d.id = lc.decision_id
                        WHERE lc.ref_text_uid IS NULL
                    ) spans
                )
                SELECT nature, slashnum, count(*)::bigint AS edges
                FROM cited
                WHERE nature IS NOT NULL AND slashnum IS NOT NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM legal_text lt
                    WHERE lt.nature = CASE WHEN cited.nature = 'reglement' THEN 'REGLEMENT' ELSE 'DIRECTIVE_EURO' END
                      AND (regexp_match(lt.title, '(\d{1,4}/\d{1,4})'))[1] = cited.slashnum
                  )
                GROUP BY nature, slashnum
                ORDER BY edges DESC
                "#,
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<_, String>(0),
                    r.get::<_, String>(1),
                    r.get::<_, i64>(2),
                )
            })
            .collect())
    }

    /// Lignes `legal_text` pour hydrater le `LinkSnapshot` du linker in-pass
    /// (ADR 0145). Tuples plats — `lj-store` ne tire pas `lj-extract` (ADR
    /// 0123 §3), la conversion vers `CatalogText` vit chez l'appelant :
    /// `(text_uid, title, title_key, nature, jurisdiction, num_prefix_agnostic,
    /// n_vigueur)`.
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "db.link_catalog_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn link_catalog_texts(
        &self,
    ) -> Result<Vec<(String, String, String, String, Option<String>, bool, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, coalesce(t.title, ''), coalesce(t.title_key, ''),
                        coalesce(t.nature, ''), t.jurisdiction,
                        coalesce(t.num_prefix_agnostic, false),
                        count(a.*) FILTER (WHERE a.status = 'VIGUEUR')
                 FROM legal_text t
                 LEFT JOIN legal_article a ON a.text_uid = t.text_uid
                 GROUP BY t.text_uid, t.title, t.title_key, t.nature, t.jurisdiction,
                          t.num_prefix_agnostic",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get(0),
                    r.get(1),
                    r.get(2),
                    r.get(3),
                    r.get(4),
                    r.get(5),
                    r.get(6),
                )
            })
            .collect())
    }

    /// Paires `(text_uid, num_key)` distinctes du catalogue d'articles —
    /// l'index d'existence du `LinkSnapshot` (`ref_num_key` jamais inventé).
    #[tracing::instrument(name = "db.link_catalog_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn link_catalog_articles(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query("SELECT DISTINCT text_uid, num_key FROM legal_article", &[])
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Recompute les arrays dénormalisés (`legal_instruments`/`legal_article_composite`,
    /// décision ET chunks) pour les `decision_id ∈ [lo, hi)` depuis `legal_citation`
    /// (migration 0098). Appelle les fonctions SQL `_sync_*_legal_instruments_for`
    /// (garde IS DISTINCT FROM : ne réécrit que les rows changés → pas de ré-index
    /// BM25 global). Lots autocommit hors transaction géante. Renvoie les comptes
    /// de lignes corrigées `(decisions, chunks)` (migration 0101) — la dérive :
    /// l'écrivain tenant write → sync atomiques, un compte non nul est le signal
    /// d'un bug d'écrivain (ADR 0147).
    #[tracing::instrument(name = "db.resync_legal_arrays_range", skip(self), fields(db.system = "postgresql"))]
    pub async fn resync_legal_arrays_range(&self, lo: i64, hi: i64) -> Result<(i64, i64)> {
        let d: i64 = self
            .conn
            .query_one(
                "SELECT _sync_decisions_legal_instruments_for(
                     ARRAY(SELECT id FROM decisions WHERE id >= $1 AND id < $2))",
                &[&lo, &hi],
            )
            .await?
            .get(0);
        let c: i64 = self
            .conn
            .query_one(
                "SELECT _sync_chunks_legal_instruments_for(
                     ARRAY(SELECT id FROM decisions WHERE id >= $1 AND id < $2))",
                &[&lo, &hi],
            )
            .await?
            .get(0);
        Ok((d, c))
    }

    /// Métadonnées `legal_text` par lot d'uids : `title_key`, `slug` et numéro
    /// « n° AA-NNN » extrait du titre (actes datés). Consommé par le banc
    /// extract (résolution des libellés attendus). Lecture seule.
    pub async fn fetch_legal_text_keys_by_uids(
        &self,
        uids: &[String],
    ) -> Result<HashMap<String, crate::repository::LegalTextMeta>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title_key, slug, \
                   (regexp_match(title, \
                     '^(?:Décret|Arrêté|Loi|Ordonnance|Décision|Règlement)[^0-9]*n[°ºo] ?(\\d{2,4}-\\d+)'))[1] \
                 FROM legal_text WHERE text_uid = ANY($1)",
                &[&uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2), r.get(3))))
            .collect())
    }

    /// Bornes `(min, max)` des `decisions.id` — pour découper le backfill des arêtes.
    pub async fn decision_id_bounds(&self) -> Result<Option<(i64, i64)>> {
        let row = self
            .conn
            .query_one("SELECT min(id), max(id) FROM decisions", &[])
            .await?;
        let lo: Option<i64> = row.get(0);
        let hi: Option<i64> = row.get(1);
        Ok(lo.zip(hi))
    }
}
