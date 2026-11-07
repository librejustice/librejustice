//! Citations à plat : une occurrence = une ligne `legal_citation`
//! `(decision_id, char_start, char_end, ref_text_uid, ref_num_key,
//! extract_version)`, écrite déjà liée par le linker in-pass de l'ingest.
//! `ref_text_uid IS NULL` = non lié ; aucun état de résolution ni override en
//! base.
//!
//! Écriture par décision / en bulk (skip-diff des sets inchangés), recompute
//! des arrays dénormalisés de facettes, et lectures du `LinkSnapshot`
//! (catalogue textes + articles).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use super::types::{CitationOccurrenceRow, CitingDecisionRow, SectionCitingTextRow};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::{ToSql, Type};

use super::referential::CITING_RECENCY_WALK_MIN_COUNT;

/// Item d'écriture bulk : `(decision_id, occurrences)`. `None` = extraction
/// sans couche citations (écrit un set vide).
pub type CitationWriteItem = (i64, Option<Vec<CitationOccurrenceRow>>);

/// Arrays de facettes d'une décision depuis ses occurrences :
/// `legal_instruments` = `ref_text_uid` distincts, `legal_article_composite` =
/// `uid|num_key` distincts — `None` (SQL NULL) si vide, comme l'`ARRAY_AGG …
/// FILTER` des fonctions de resync. Tri bytewise = `COLLATE "C"`.
/// Les occurrences `suivants` liées apportent leur famille via `families` — la
/// même `_suivants_family_keys` que les fonctions de resync, datée à `date` (la
/// date de lecture de la décision).
fn citation_facet_arrays(
    rows: &[CitationOccurrenceRow],
    families: &HashMap<(String, String, NaiveDate), Vec<String>>,
    date: NaiveDate,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let mut instruments: Vec<String> = rows.iter().filter_map(|o| o.ref_text_uid.clone()).collect();
    instruments.sort_unstable();
    instruments.dedup();
    let mut composite: Vec<String> = rows
        .iter()
        .flat_map(|o| {
            let Some((uid, num)) = o.ref_text_uid.as_deref().zip(o.ref_num_key.as_deref()) else {
                return Vec::new();
            };
            let mut keys = vec![format!("{uid}|{num}")];
            if o.suivants {
                if let Some(fam) = families.get(&(uid.to_string(), num.to_string(), date)) {
                    keys.extend(fam.iter().map(|k| format!("{uid}|{k}")));
                }
            }
            keys
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
/// ref_text_uid, ref_num_key, suivants)` — les éléments positionnels du blob
/// `spans`.
type OccurrenceFingerprint = (i32, i32, Option<String>, Option<String>, bool);

impl DecisionRepository<'_> {
    /// (Ré)écrit la couche citations d'UNE décision — sucre sur
    /// [`Self::replace_citations_bulk`], l'unique chemin d'écriture recognizer
    /// (garde jamais-dégrader, skip-diff, séquence write → sync).
    pub async fn replace_citations(
        &self,
        decision_id: i64,
        occurrences: Option<&[CitationOccurrenceRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_citations_bulk(&[(decision_id, occurrences.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit la couche citations d'un lot de décisions : garde jamais-dégrader
    /// (une décision à version > `EXTRACT_VERSION` — révision manuelle — n'est
    /// jamais remplacée par le recognizer), skip côté Rust des décisions au set
    /// d'occurrences inchangé, puis DELETE + COPY dans `legal_citation` et
    /// recompute des arrays de facettes (dans cet ordre — les arrays dérivent de
    /// `legal_citation`). Idempotent. Rend les décisions RÉELLEMENT réécrites, pour
    /// que l'appelant date leur page du jour.
    #[tracing::instrument(name = "db.replace_citations_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_citations_bulk(&self, items: &[CitationWriteItem]) -> Result<Vec<i64>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        // Invariant : jamais remplacer la couche citations d'une décision à
        // version > EXTRACT_VERSION (révision manuelle).
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let protected: HashSet<i64> = self
            .conn
            .query(
                "SELECT id FROM decisions WHERE id = ANY($1) AND extract_version > $2",
                &[&ids, &lj_versions::EXTRACT_VERSION],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        let mut items = self.filter_unchanged_citations(items).await?;
        items.retain(|(id, _)| !protected.contains(id));
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let changed: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let v9_items: Vec<(i64, &[CitationOccurrenceRow])> = items
            .iter()
            .map(|(id, occ)| (*id, occ.as_deref().unwrap_or(&[])))
            .collect();
        self.write_citation_occurrences(&v9_items, lj_versions::EXTRACT_VERSION)
            .await?;
        Ok(changed)
    }

    /// Sous-ensemble des items à réécrire (skip-diff du re-extract). Une décision
    /// est « changée » si son set d'occurrences `(char_start, char_end,
    /// ref_text_uid, ref_num_key)` diffère des lignes `legal_citation` à
    /// version ≤ courante de la DB — spans EUX-MÊMES comparés, pas seulement les
    /// cibles. Converge : après réécriture, l'empreinte DB == l'empreinte extraite
    /// → ne re-déclenche pas.
    pub(super) async fn filter_unchanged_citations(
        &self,
        items: &[CitationWriteItem],
    ) -> Result<Vec<CitationWriteItem>> {
        let decision_ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let db_rows = self
            .conn
            .query(
                "
                SELECT decision_id, spans
                FROM legal_citation
                WHERE decision_id = ANY($1) AND extract_version <= $2
                ",
                &[&decision_ids, &lj_versions::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<OccurrenceFingerprint>> = HashMap::new();
        for row in &db_rows {
            let spans: serde_json::Value = row.get(1);
            let set = current.entry(row.get(0)).or_default();
            for el in spans.as_array().expect("spans : tableau jsonb") {
                let el = el.as_array().expect("span : tableau positionnel");
                set.insert((
                    el[0].as_i64().expect("char_start") as i32,
                    el[1].as_i64().expect("char_end") as i32,
                    el[2].as_str().map(str::to_owned),
                    el[3].as_str().map(str::to_owned),
                    el[4].as_bool().expect("suivants"),
                ));
            }
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
                        o.suivants,
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
    /// décisions passées, depuis leurs arêtes fraîchement réécrites. Appelé par
    /// [`Self::write_citation_occurrences`] dans le même flux d'écriture. Les
    /// agrégats DISTINCT se calculent en Rust depuis les occurrences déjà en
    /// mémoire (tri bytewise = COLLATE "C", même ordre que les fonctions SQL de
    /// resync). Les UPDATE gardent `IS DISTINCT FROM` (zéro churn si rien ne
    /// change).
    async fn sync_citation_facet_arrays(
        &self,
        items: &[(i64, &[CitationOccurrenceRow])],
    ) -> Result<()> {
        let dates = self.reading_dates(items).await?;
        let families = self.suivants_families(items, &dates).await?;
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> =
            Vec::with_capacity(items.len() * 3);
        let mut value_rows: Vec<String> = Vec::with_capacity(items.len());
        for (pos, (decision_id, rows)) in items.iter().enumerate() {
            let (instruments, composite) =
                citation_facet_arrays(rows, &families, dates[decision_id]);
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

    /// Écrit la couche citations d'un lot de décisions : DELETE des blobs
    /// existants, **COPY binaire** d'une ligne jsonb par décision porteuse dans
    /// `legal_citation`, puis recompute des arrays de facettes dérivés — la paire
    /// write → sync est scellée ICI et nulle part ailleurs. **La transaction
    /// appartient à l'appelant** (mod.rs : le pipeline enveloppe ses batches) —
    /// aucun BEGIN/COMMIT ici : un COMMIT imbriqué commiterait la transaction
    /// externe prématurément et désatomiserait write ↔ sync. `extract_version` =
    /// `EXTRACT_VERSION` (recognizer, jamais appelé sur une décision à version
    /// supérieure — garde décision-niveau amont) ou toute version explicite
    /// (`replace_citations_at_version`, remplace tout). Asserts durs à l'entrée
    /// (tri strict par `char_start`, zéro chevauchement, `char_end > char_start`,
    /// `ref_num_key ⇒ ref_text_uid`) — l'amont les garantit, une violation ici est
    /// un bug.
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
                "COPY legal_citation (decision_id, extract_version, spans) \
                 FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(sink, &[Type::INT8, Type::INT2, Type::JSONB]);
        tokio::pin!(writer);
        for (decision_id, rows) in items {
            // Un set vide ne s'écrit pas : décision sans citations = absence
            // de ligne (le fingerprint du skip-diff compare au set vide).
            if rows.is_empty() {
                continue;
            }
            let spans = serde_json::Value::Array(
                rows.iter()
                    .map(|o| {
                        serde_json::json!([
                            o.char_start,
                            o.char_end,
                            o.ref_text_uid,
                            o.ref_num_key,
                            o.suivants
                        ])
                    })
                    .collect(),
            );
            writer
                .as_mut()
                .write(&[decision_id, &extract_version, &spans])
                .await?;
        }
        writer.finish().await?;
        self.sync_citation_facet_arrays(items).await?;
        Ok(())
    }

    /// Date de lecture des décisions du lot, `CURRENT_DATE` à défaut : la borne
    /// qui date les familles « et suivants ». Lecture par PK.
    async fn reading_dates(
        &self,
        items: &[(i64, &[CitationOccurrenceRow])],
    ) -> Result<HashMap<i64, NaiveDate>> {
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        Ok(self
            .conn
            .query(
                "SELECT id, coalesce(date_lecture, CURRENT_DATE) \
                 FROM decisions WHERE id = ANY($1)",
                &[&ids],
            )
            .await?
            .into_iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect())
    }

    /// Familles « et suivants » des occurrences liées porteuses du signal : une
    /// entrée `(uid, num_key, date) → num_keys` par ancre expansable —
    /// `_suivants_family_keys` porte les garde-fous (section unique, arête en
    /// vigueur à la date, cap 20 ; NULL sinon) et reste la source partagée avec
    /// les fonctions de resync. La date entre dans la clé : deux décisions de
    /// dates différentes citant la même ancre n'ont pas la même famille. Zéro
    /// requête quand aucun span `suivants`.
    async fn suivants_families(
        &self,
        items: &[(i64, &[CitationOccurrenceRow])],
        dates: &HashMap<i64, NaiveDate>,
    ) -> Result<HashMap<(String, String, NaiveDate), Vec<String>>> {
        let mut triples: Vec<(String, String, NaiveDate)> = Vec::new();
        for (decision_id, rows) in items {
            let date = *dates.get(decision_id).ok_or_else(|| {
                StoreError::Invalid(format!("décision {decision_id} absente de `decisions`"))
            })?;
            triples.extend(
                rows.iter()
                    .filter(|o| o.suivants)
                    .filter_map(|o| Some((o.ref_text_uid.clone()?, o.ref_num_key.clone()?, date))),
            );
        }
        triples.sort_unstable();
        triples.dedup();
        if triples.is_empty() {
            return Ok(HashMap::new());
        }
        let mut uids: Vec<String> = Vec::with_capacity(triples.len());
        let mut nums: Vec<String> = Vec::with_capacity(triples.len());
        let mut days: Vec<NaiveDate> = Vec::with_capacity(triples.len());
        for (uid, num, date) in triples {
            uids.push(uid);
            nums.push(num);
            days.push(date);
        }
        let rows = self
            .conn
            .query(
                "SELECT p.uid, p.num, p.d, _suivants_family_keys(p.uid, p.num, p.d) \
                 FROM unnest($1::text[], $2::text[], $3::date[]) AS p(uid, num, d)",
                &[&uids, &nums, &days],
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let fam: Option<Vec<String>> = r.get(3);
                Some(((r.get(0), r.get(1), r.get(2)), fam?))
            })
            .collect())
    }

    /// Slashnums UE (année/séquence) **cités mais absents** du catalogue, pour piloter
    /// l'ingestion EUR-Lex. Renvoie `(nature, slashnum, edges)` où `nature ∈
    /// {reglement, directive}`, trié par mentions décroissantes (une ligne
    /// `legal_citation` non liée = une mention). « Absent » = aucun `legal_text` de
    /// la nature correspondante dont le `title` porte ce slashnum. Lecture seule.
    #[tracing::instrument(name = "db.cited_eu_slashnums_missing", skip(self), fields(db.system = "postgresql"))]
    pub async fn cited_eu_slashnums_missing(&self) -> Result<Vec<(String, String, i64)>> {
        // Les clés de capture ne sont pas stockées — la forme citée se retranche de
        // `full_text` par les spans **non liés** de `legal_citation` (offsets
        // codepoints = sémantique caractère de `substring`). Scan lourd (détoaste
        // chaque décision porteuse) : commande manuelle rare.
        let rows = self
            .conn
            .query(
                &format!(
                r#"
                WITH cited AS (
                    SELECT
                        CASE WHEN raw ~* '^\s*(?:le |la |les |l['''']? ?|du |de la |de l['''']? ?)?\s*r[eè]glement' THEN 'reglement'
                             WHEN raw ~* '^\s*(?:le |la |les |l['''']? ?|du |de la |de l['''']? ?)?\s*directive' THEN 'directive' END AS nature,
                        (regexp_match(raw,
                            '(?i)^\s*(?:le |la |les |l['''']? ?|du |de la |de l['''']? ?)?\s*(?:directive|r[eè]glement)[^0-9]{{0,40}}(\d{{1,4}}/\d{{1,4}})'))[1] AS slashnum
                    FROM (
                        SELECT substring({text} FROM (el->>0)::int + 1
                                         FOR (el->>1)::int - (el->>0)::int) AS raw
                        FROM legal_citation lc
                        JOIN decisions d ON d.id = lc.decision_id
                        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
                        WHERE el->>2 IS NULL
                    ) spans
                )
                SELECT nature, slashnum, count(*)::bigint AS edges
                FROM cited
                WHERE nature IS NOT NULL AND slashnum IS NOT NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM legal_text lt
                    WHERE lt.nature = CASE WHEN cited.nature = 'reglement' THEN 'REGLEMENT' ELSE 'DIRECTIVE_EURO' END
                      AND (regexp_match(lt.title, '(\d{{1,4}}/\d{{1,4}})'))[1] = cited.slashnum
                  )
                GROUP BY nature, slashnum
                ORDER BY edges DESC
                "#,
                text = super::pick::text("p.full_text", "d")
            ),
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

    /// Lignes `legal_text` pour hydrater le `LinkSnapshot` du linker in-pass.
    /// Tuples plats, la conversion vers `CatalogText` vit chez l'appelant :
    /// `(text_uid, title, title_key, nature, jurisdiction, num_prefix_agnostic,
    /// n_vigueur)`.
    ///
    /// L'agrégat `legal_text × legal_article` scanne toute `legal_article`
    /// (lignes larges) et dépasse le `statement_timeout` du pool (30 s) à froid
    /// sur le corpus de prod : levé localement dans une transaction dédiée (même
    /// pattern que `iter_entities_for_sitemap`). Chargement one-shot d'ingest, pas
    /// de chemin requête utilisateur.
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "db.link_catalog_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn link_catalog_texts(
        &self,
    ) -> Result<
        Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            bool,
            i64,
            Option<String>,
            Option<String>,
        )>,
    > {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<_>> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let (valid_a, norm_a) = (
                super::referential::article_valid_now("a"),
                super::referential::article_is_norm("a"),
            );
            let rows = self
                .conn
                .query(
                    &format!(
                        "SELECT t.text_uid, coalesce(t.title, ''), coalesce(t.title_key, ''),
                            coalesce(t.nature, ''), t.jurisdiction,
                            coalesce(t.num_prefix_agnostic, false),
                            count(a.*) FILTER (WHERE {valid_a} AND {norm_a}),
                            t.date_texte::text, t.nor
                     FROM legal_text t
                     LEFT JOIN legal_article a ON a.text_uid = t.text_uid
                     GROUP BY t.text_uid, t.title, t.title_key, t.nature, t.jurisdiction,
                              t.num_prefix_agnostic, t.date_texte, t.nor"
                    ),
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
                        r.get(7),
                        r.get(8),
                    )
                })
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
    /// décision ET chunks) pour les `decision_id ∈ [lo, hi)` depuis `legal_citation`.
    /// Appelle les fonctions SQL `_sync_*_legal_instruments_for` (garde IS DISTINCT
    /// FROM : ne réécrit que les rows changés → pas de ré-index BM25 global). Lots
    /// autocommit hors transaction géante. Renvoie les comptes de lignes corrigées
    /// `(decisions, chunks)` — la dérive : l'écrivain tenant write → sync atomiques,
    /// un compte non nul est le signal d'un bug d'écrivain.
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

    /// Rebuild intégral de `citing_decision_counts` : décomptes de décisions
    /// citantes par terme cité (alphabet `lj_cit_terms` — `uid` ou `uid|num`). Une
    /// seule instruction (CTE modifiantes, même snapshot) : upsert des termes
    /// recomptés + suppression des termes disparus, garde `<>` pour ne réécrire
    /// que les décomptes changés. Renvoie le nombre de termes upsertés.
    #[tracing::instrument(name = "db.rebuild_citing_decision_counts", skip(self), fields(db.system = "postgresql"))]
    pub async fn rebuild_citing_decision_counts(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                WITH fresh AS (
                    SELECT term AS cited_term, count(*) AS decision_count
                    FROM legal_citation, unnest(public.lj_cit_terms(spans)) AS term
                    GROUP BY term
                ), stale AS (
                    DELETE FROM citing_decision_counts c
                    WHERE NOT EXISTS (
                        SELECT 1 FROM fresh f WHERE f.cited_term = c.cited_term)
                )
                INSERT INTO citing_decision_counts (cited_term, decision_count)
                SELECT cited_term, decision_count FROM fresh
                ON CONFLICT (cited_term) DO UPDATE
                SET decision_count = excluded.decision_count
                WHERE citing_decision_counts.decision_count
                      <> excluded.decision_count
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Décomptes de décisions citantes d'un ensemble d'articles d'un même texte
    /// (`citing_decision_counts` — clé `text_uid || '|' || num_key`), rendus
    /// **triés par décompte décroissant** (`num_key` en second pour un ordre total,
    /// donc un palmarès stable à décomptes égaux). Les articles sans décompte sont
    /// absents : aucun décompte = aucune décision citante.
    #[tracing::instrument(name = "db.citing_counts_for_articles", skip(self, num_keys), fields(db.system = "postgresql", keys = num_keys.len()))]
    pub async fn citing_counts_for_articles(
        &self,
        text_uid: &str,
        num_keys: &[String],
    ) -> Result<Vec<(String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT k.num_key, c.decision_count \
                 FROM unnest($2::text[]) AS k(num_key) \
                 JOIN citing_decision_counts c \
                   ON c.cited_term = $1 || '|' || k.num_key \
                 ORDER BY c.decision_count DESC, k.num_key",
                &[&text_uid, &num_keys],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Palmarès « articles les plus cités » d'un **texte entier** (colonne droite
    /// de `/norm/{juris}/{code}`) : `(num_key, numéro affichable, décompte)`, du plus cité
    /// au moins cité.
    ///
    /// Le classement se lit dans `citing_decision_counts` seul — un scan de la
    /// table de compteurs filtré au préfixe du texte, indépendant de la taille du
    /// code. Le filtre est un `LIKE 'uid|%'` et non une borne d'intervalle :
    /// l'ordre de la collation n'est pas l'ordre des octets, et `< 'uid}'` rend
    /// zéro ligne.
    ///
    /// Les `candidates` premiers termes du palmarès sont ensuite joints à
    /// `legal_article` pour leur numéro affichable, ce qui écarte les articles
    /// disparus du texte — d'où un vivier plus large que le `limit` demandé.
    #[tracing::instrument(name = "db.top_cited_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn top_cited_articles(
        &self,
        text_uid: &str,
        limit: i64,
        candidates: i64,
    ) -> Result<Vec<(String, String, i64)>> {
        let rows = self
            .conn
            .query(
                "WITH top AS ( \
                     SELECT substring(c.cited_term FROM length($1) + 2) AS num_key, \
                            c.decision_count \
                     FROM citing_decision_counts c \
                     WHERE c.cited_term LIKE $1 || '|%' \
                     ORDER BY c.decision_count DESC \
                     LIMIT $3 \
                 ), labelled AS ( \
                     SELECT DISTINCT ON (t.num_key) t.num_key, a.num, t.decision_count \
                     FROM top t \
                     JOIN legal_article a \
                       ON a.text_uid = $1 AND a.num_key = t.num_key \
                     ORDER BY t.num_key, a.date_debut DESC \
                 ) \
                 SELECT num_key, num, decision_count FROM labelled \
                 ORDER BY decision_count DESC, num_key \
                 LIMIT $2",
                &[&text_uid, &limit, &candidates],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Textes citant **au moins `min_articles`** des articles `num_keys` d'un
    /// texte — les textes citants d'une division, agrégés au grain du texte citant
    /// et classés par nombre d'articles visés. Le texte lui-même est écarté : ses
    /// renvois internes pointeraient la page qu'on lit.
    ///
    /// Lecture par le **côté cible** de l'arête (`idx_legal_link_target_art`), le
    /// citant étant alors `owner_text_uid` — un uid, pas un libellé à découper.
    /// DILA écrit les deux copies d'une citation ; celle-ci est la seule qui donne
    /// le citant.
    ///
    /// `direction = 'outgoing'` = « le propriétaire cite la cible » : le fichier du
    /// citant porte `sens=source`.
    ///
    /// Seul le verbe `cite` est agrégé : les modificatifs ont leur propre bloc sur
    /// la page article.
    ///
    /// Titres résolus **après** le `LIMIT`, sur les seules lignes rendues.
    #[tracing::instrument(name = "db.section_citing_texts", skip(self, num_keys), fields(db.system = "postgresql", keys = num_keys.len()))]
    pub async fn section_citing_texts(
        &self,
        text_uid: &str,
        num_keys: &[String],
        excluded_natures: &[String],
        min_articles: i64,
        limit: i64,
    ) -> Result<Vec<SectionCitingTextRow>> {
        let rows = self
            .conn
            .query(
                "WITH citing AS ( \
                     SELECT l.owner_text_uid AS uid, \
                            count(DISTINCT l.target_num_key) AS articles \
                     FROM legal_link l \
                     WHERE l.target_text_uid = $1 AND l.target_num_key = ANY($2) \
                       AND l.direction = 'outgoing' AND l.verb = 'cite' \
                       AND l.owner_text_uid <> $1 \
                       AND NOT EXISTS (SELECT 1 FROM legal_text o \
                                       WHERE o.text_uid = l.owner_text_uid \
                                         AND upper(o.nature) = ANY($5)) \
                     GROUP BY 1 \
                     HAVING count(DISTINCT l.target_num_key) >= $3 \
                     ORDER BY articles DESC, uid \
                     LIMIT $4 \
                 ) \
                 SELECT t.title AS label, t.slug, t.jurisdiction, c.articles \
                 FROM citing c \
                 JOIN legal_text t ON t.text_uid = c.uid \
                 ORDER BY c.articles DESC, label",
                &[
                    &text_uid,
                    &num_keys,
                    &min_articles,
                    &limit,
                    &excluded_natures,
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| SectionCitingTextRow {
                label: r.get(0),
                text_slug: r.get(1),
                text_jurisdiction: r.get(2),
                article_count: r.get(3),
            })
            .collect())
    }

    /// Décisions citant **au moins un** des articles `num_keys` d'un texte — les
    /// citantes d'une division, qui agrège son sous-arbre. Même forme de ligne,
    /// même pagination et même tri que les citantes d'un article
    /// ([`Self::law_decisions_citing`]) : portée d'abord, puis date décroissante.
    ///
    /// **Dédupliqué par construction** : `legal_citation` porte une ligne blob
    /// par décision, donc citer trois articles de la division sort une fois,
    /// sans `DISTINCT`. Les trois ressortent dans `cited_num_keys`.
    ///
    /// Deux plans selon le volume d'union :
    /// - union courante : bitmap GIN (`&&` = OR des postings, indexable) ;
    /// - union ≥ [`CITING_RECENCY_WALK_MIN_COUNT`] : marche descendante de
    ///   `idx_decisions_date_lecture`, récence seule — trier par portée
    ///   exigerait de visiter tous les postings.
    ///
    /// Pas de fenêtre de validité : une division n'en a pas, chacun de ses
    /// articles a la sienne. La date de consultation choisit la *composition* du
    /// sous-arbre, pas la fenêtre des décisions.
    #[tracing::instrument(name = "db.law_decisions_citing_any", skip(self, num_keys), fields(db.system = "postgresql", keys = num_keys.len()))]
    pub async fn law_decisions_citing_any(
        &self,
        text_uid: &str,
        num_keys: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CitingDecisionRow>> {
        let terms: Vec<String> = num_keys.iter().map(|k| format!("{text_uid}|{k}")).collect();
        let citing_count: i64 = self
            .conn
            .query_one(
                // `sum(bigint)` rend un `numeric` : la somme se relit en i64
                // seulement castée (le décompte par article, lui, est bigint).
                "SELECT coalesce(sum(decision_count), 0)::bigint FROM citing_decision_counts \
                 WHERE cited_term = ANY($1)",
                &[&terms],
            )
            .await?
            .get(0);
        let params: [&(dyn ToSql + Sync); 3] = [&terms, &limit, &offset];
        let rows = if citing_count >= CITING_RECENCY_WALK_MIN_COUNT {
            // Marche par récence : l'EXISTS sur le blob n'est PAS indexable
            // (pas de `&&` sur l'expression GIN) — le planner ne peut que descendre
            // l'index date, coût ≈ limit × N/df itérations, d'autant plus faible que
            // l'union est large. `d.date_lecture IS NOT NULL` écarte les décisions sans
            // date : le scan arrière de l'index sert l'ORDER BY sans tri de tête.
            self.conn
                .query(
                    &format!(
                        "
                    SELECT d.id, d.public_id, d.jurisdiction_type,
                           j.label, d.date_lecture::text, d.docket_numbers,
                           d.publication_codes, {summary},
                           ARRAY(SELECT DISTINCT el->>3
                                 FROM jsonb_array_elements(lc.spans) AS el
                                 WHERE (el->>2) || '|' || (el->>3) = ANY($1)),
                           d.jurisdiction_code
                    FROM decisions d
                    JOIN legal_citation lc ON lc.decision_id = d.id
                    LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                    WHERE d.date_lecture IS NOT NULL
                      AND EXISTS (
                          SELECT 1 FROM jsonb_array_elements(lc.spans) AS el
                          WHERE (el->>2) || '|' || (el->>3) = ANY($1))
                    ORDER BY d.date_lecture DESC, d.id
                    LIMIT $2 OFFSET $3
                    ",
                        summary = super::pick::text("p.summary", "d")
                    ),
                    &params,
                )
                .await?
        } else {
            // Autorité d'abord, puis date décroissante. Les paliers sont des
            // constantes du code, inlinées, pas des données.
            let rank_expr = super::support::authority_rank_sql("d.publication_codes");
            self.conn
                .query(
                    &format!(
                        "
                        SELECT d.id, d.public_id, d.jurisdiction_type,
                               j.label, d.date_lecture::text, d.docket_numbers,
                               d.publication_codes, {summary},
                               ARRAY(SELECT DISTINCT split_part(t, '|', 2)
                                     FROM unnest(public.lj_cit_terms(lc.spans)) t
                                     WHERE t = ANY($1)),
                               {rank_expr} AS authority_rank,
                               d.jurisdiction_code
                        FROM legal_citation lc
                        JOIN decisions d ON d.id = lc.decision_id
                        LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                        WHERE public.lj_cit_terms(lc.spans) && $1::text[]
                        -- Texte ISO 'YYYY-MM-DD' → tri == chronologique.
                        ORDER BY authority_rank, d.date_lecture::text DESC NULLS LAST, d.id
                        LIMIT $2 OFFSET $3
                        ",
                        summary = super::pick::text("p.summary", "d")
                    ),
                    &params,
                )
                .await?
        };
        Ok(rows
            .iter()
            .map(|r| CitingDecisionRow {
                id: r.get(0),
                public_id: r.get(1),
                jurisdiction_type: r.get(2),
                jurisdiction_name: r.get(3),
                jurisdiction_code: r.get("jurisdiction_code"),
                date_lecture: r.get(4),
                docket_numbers: r.get(5),
                publication_codes: r.get(6),
                summary: r.get(7),
                cited_num_keys: r.get(8),
            })
            .collect())
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

    /// (Ré)écrit les renvois norme→article d'un TEXTE du référentiel : DELETE +
    /// COPY au grain `owner_text_uid` (corps + toutes ses versions d'articles d'un
    /// coup — la passe traite le texte entier). Idempotent.
    pub async fn replace_text_legal_citations(
        &self,
        owner_text_uid: &str,
        rows: &[super::types::TextLegalCitationRow],
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM text_legal_citation WHERE owner_text_uid = $1",
                &[&owner_text_uid],
            )
            .await?;
        let rows: Vec<_> = rows.iter().map(|r| (owner_text_uid, r)).collect();
        self.copy_text_legal_citations(&rows).await
    }

    /// COPY of `text_legal_citation` rows, each beside its owner `text_uid`.
    pub(super) async fn copy_text_legal_citations(
        &self,
        rows: &[(&str, &super::types::TextLegalCitationRow)],
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let sink = self
            .conn
            .copy_in(
                "COPY text_legal_citation (owner_text_uid, owner_num_key, owner_date_debut, \
                 char_start, char_end, ref_text_uid, ref_num_key, extract_version) \
                 FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::INT4,
                Type::INT4,
                Type::TEXT,
                Type::TEXT,
                Type::INT2,
            ],
        );
        tokio::pin!(writer);
        for (owner_text_uid, r) in rows {
            writer
                .as_mut()
                .write(&[
                    &owner_text_uid,
                    &r.owner_num_key,
                    &r.owner_date_debut,
                    &r.char_start,
                    &r.char_end,
                    &r.ref_text_uid,
                    &r.ref_num_key,
                    &lj_versions::EXTRACT_VERSION,
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
    }

    /// Oracle DILA des renvois article→article : ce que chaque propriétaire
    /// DÉCLARE citer (`legal_link`, bloc `<LIENS>` de la source), au grain
    /// (propriétaire, article, numéro cité). Sert à départager le CODE quand
    /// l'extracteur hésite entre deux textes portant le même numéro — la source
    /// sait de quel décret du même jour elle parle.
    ///
    /// Dédoublonné en SQL. Les clés citées par PLUSIEURS codes depuis le même
    /// article sortent aussi, à l'appelant de constater l'ambiguïté et de
    /// s'abstenir.
    pub async fn declared_article_cites(&self) -> Result<Vec<(String, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT owner_text_uid, COALESCE(owner_num_key, ''), \
                        target_num_key, target_text_uid \
                 FROM legal_link \
                 WHERE typelien = 'CITATION' AND verb = 'cite' \
                   AND direction = 'outgoing' \
                   AND target_text_uid IS NOT NULL \
                   AND target_num_key IS NOT NULL AND target_num_key <> ''",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Volet banc : couverture et accord de code des spans du corps face à
    /// l'oracle des renvois déclarés. Trois anti-jointures hachées sur des couples
    /// dédoublonnés — offline, plusieurs minutes.
    pub async fn text_refs_oracle_stats(&self) -> Result<super::types::TextRefsOracleStats> {
        // Les deux CTE sont partagées par les trois mesures, mais les clés de
        // jointure diffèrent (couple complet vs numéro seul) : trois passes.
        const EDGES: &str = "SELECT DISTINCT owner_text_uid, COALESCE(owner_num_key, '') AS onk, \
                                    target_num_key, target_text_uid \
                             FROM legal_link \
                             WHERE typelien = 'CITATION' AND verb = 'cite' \
                               AND direction = 'outgoing' \
                               AND target_text_uid IS NOT NULL \
                               AND target_num_key IS NOT NULL AND target_num_key <> ''";
        const SPANS: &str = "SELECT DISTINCT owner_text_uid, COALESCE(owner_num_key, '') AS onk, \
                                    ref_num_key, ref_text_uid \
                             FROM text_legal_citation \
                             WHERE ref_text_uid IS NOT NULL AND ref_num_key IS NOT NULL";

        let coverage = self
            .conn
            .query_one(
                &format!(
                    "WITH edges AS ({EDGES}), spans AS ({SPANS}) \
                     SELECT count(*), count(s.onk), \
                            count(*) FILTER (WHERE s.onk IS NULL \
                                             AND e.owner_text_uid = e.target_text_uid), \
                            count(*) FILTER (WHERE s.onk IS NULL \
                                             AND e.owner_text_uid <> e.target_text_uid) \
                     FROM edges e \
                     LEFT JOIN spans s ON s.owner_text_uid = e.owner_text_uid \
                                      AND s.onk = e.onk \
                                      AND s.ref_text_uid = e.target_text_uid \
                                      AND s.ref_num_key = e.target_num_key"
                ),
                &[],
            )
            .await?;
        // Accord = la cible du span est PARMI celles déclarées pour ce numéro,
        // pas « égale à l'une d'elles prise au hasard » : sur une clé que la source
        // déclare vers deux textes, l'extracteur qui en choisit un a raison.
        let agreement = self
            .conn
            .query_one(
                &format!(
                    "WITH edges AS ({EDGES}), spans AS ({SPANS}), \
                          declared_keys AS (SELECT DISTINCT owner_text_uid, onk, target_num_key \
                                            FROM edges), \
                          comparable AS (SELECT s.* FROM spans s \
                                         JOIN declared_keys k \
                                           ON k.owner_text_uid = s.owner_text_uid \
                                          AND k.onk = s.onk \
                                          AND k.target_num_key = s.ref_num_key) \
                     SELECT count(*), count(e.target_text_uid) \
                     FROM comparable c \
                     LEFT JOIN edges e ON e.owner_text_uid = c.owner_text_uid \
                                      AND e.onk = c.onk \
                                      AND e.target_num_key = c.ref_num_key \
                                      AND e.target_text_uid = c.ref_text_uid"
                ),
                &[],
            )
            .await?;
        let ambiguous = self
            .conn
            .query_one(
                &format!(
                    "WITH edges AS ({EDGES}) \
                     SELECT count(*) FROM ( \
                         SELECT owner_text_uid, onk, target_num_key \
                         FROM edges GROUP BY 1, 2, 3 \
                         HAVING count(DISTINCT target_text_uid) > 1) a"
                ),
                &[],
            )
            .await?;
        Ok(super::types::TextRefsOracleStats {
            declared: coverage.get(0),
            with_span: coverage.get(1),
            missing_internal: coverage.get(2),
            missing_external: coverage.get(3),
            compared: agreement.get(0),
            agree: agreement.get(1),
            ambiguous: ambiguous.get(0),
        })
    }

    /// Spans de renvoi de la version d'article servie, triés par `char_start`,
    /// joints au catalogue (slug/titre de la cible + gate « la fiche est lisible »
    /// pour la mention nue).
    /// The citation spans of several article versions of one text, by
    /// `(num_key, date_debut)`, each in order.
    #[tracing::instrument(name = "db.articles_citation_spans", skip_all, fields(db.system = "postgresql", owners = owners.len()))]
    pub async fn articles_citation_spans(
        &self,
        owner_text_uid: &str,
        owners: &[(String, chrono::NaiveDate)],
    ) -> Result<
        std::collections::HashMap<
            (String, chrono::NaiveDate),
            Vec<super::types::ArticleCitationSpanRow>,
        >,
    > {
        let (keys, dates): (Vec<&str>, Vec<chrono::NaiveDate>) =
            owners.iter().map(|(k, d)| (k.as_str(), *d)).unzip();
        let rows = self
            .conn
            .query(
                "SELECT o.num_key, o.date_debut, c.char_start, c.char_end, lt.slug, \
                        c.ref_num_key, lt.title, lt.jurisdiction, \
                        (EXISTS (SELECT 1 FROM legal_article a \
                                 WHERE a.text_uid = lt.text_uid) \
                         OR coalesce(length(lt.body), 0) > 0) AS readable \
                 FROM unnest($2::text[], $3::date[]) AS o(num_key, date_debut) \
                 JOIN text_legal_citation c \
                   ON c.owner_text_uid = $1 \
                  AND COALESCE(c.owner_num_key, '') = o.num_key \
                  AND COALESCE(c.owner_date_debut, '0001-01-01'::date) = o.date_debut \
                 JOIN legal_text lt ON lt.text_uid = c.ref_text_uid \
                 ORDER BY c.char_start",
                &[&owner_text_uid, &keys, &dates],
            )
            .await?;
        let mut out: std::collections::HashMap<_, Vec<_>> = std::collections::HashMap::new();
        for r in &rows {
            out.entry((r.get::<_, String>(0), r.get::<_, chrono::NaiveDate>(1)))
                .or_default()
                .push(super::types::ArticleCitationSpanRow {
                    char_start: r.get(2),
                    char_end: r.get(3),
                    ref_slug: r.get(4),
                    ref_num_key: r.get(5),
                    ref_title: r.get(6),
                    ref_jurisdiction: r.get(7),
                    ref_readable: r.get(8),
                });
        }
        Ok(out)
    }

    pub async fn article_citation_spans(
        &self,
        owner_text_uid: &str,
        owner_num_key: &str,
        owner_date_debut: chrono::NaiveDate,
    ) -> Result<Vec<super::types::ArticleCitationSpanRow>> {
        let rows = self
            .conn
            .query(
                "SELECT c.char_start, c.char_end, lt.slug, c.ref_num_key, lt.title, \
                        lt.jurisdiction, \
                        (EXISTS (SELECT 1 FROM legal_article a \
                                 WHERE a.text_uid = lt.text_uid) \
                         OR coalesce(length(lt.body), 0) > 0) AS readable \
                 FROM text_legal_citation c \
                 JOIN legal_text lt ON lt.text_uid = c.ref_text_uid \
                 WHERE c.owner_text_uid = $1 \
                   AND COALESCE(c.owner_num_key, '') = $2 \
                   AND COALESCE(c.owner_date_debut, '0001-01-01'::date) = $3 \
                 ORDER BY c.char_start",
                &[&owner_text_uid, &owner_num_key, &owner_date_debut],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| super::types::ArticleCitationSpanRow {
                char_start: r.get(0),
                char_end: r.get(1),
                ref_slug: r.get(2),
                ref_num_key: r.get(3),
                ref_title: r.get(4),
                ref_jurisdiction: r.get(5),
                ref_readable: r.get(6),
            })
            .collect())
    }
}
