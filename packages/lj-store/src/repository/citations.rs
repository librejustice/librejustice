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
/// Les occurrences `suivants` liées apportent leur famille (ADR 0226) via
/// `families` — la même `_suivants_family_keys` que les fonctions de resync.
fn citation_facet_arrays(
    rows: &[CitationOccurrenceRow],
    families: &HashMap<(String, String), Vec<String>>,
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
                if let Some(fam) = families.get(&(uid.to_string(), num.to_string())) {
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
/// `spans` (ADR 0247 ; ADR 0226 — le signal famille).
type OccurrenceFingerprint = (i32, i32, Option<String>, Option<String>, bool);

impl DecisionRepository<'_> {
    /// Paires `(text_uid, clé d'article)` de la denylist procédurale
    /// (`lj_core::procedural`) résolues contre le catalogue. Depuis
    /// l'ADR 0250 les citations procédurales SE STOCKENT ; ne sert plus que
    /// la purge one-shot inverse (`purge-procedural-citations`), conservée
    /// jusqu'à validation du backfill puis supprimée.
    pub async fn procedural_ref_pairs(&self) -> Result<HashSet<(String, String)>> {
        let slugs: Vec<&str> = lj_core::procedural::PROCEDURAL_ARTICLE_DENYLIST
            .iter()
            .map(|(s, _)| *s)
            .collect();
        let rows = self
            .conn
            .query(
                "SELECT slug, text_uid FROM legal_text WHERE slug = ANY($1)",
                &[&slugs],
            )
            .await?;
        let uid_by_slug: HashMap<String, String> = rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
            .collect();
        let mut pairs = HashSet::new();
        for (slug, keys) in lj_core::procedural::PROCEDURAL_ARTICLE_DENYLIST {
            if let Some(uid) = uid_by_slug.get(*slug) {
                for key in *keys {
                    pairs.insert((uid.clone(), key.to_string()));
                }
            }
        }
        Ok(pairs)
    }

    /// Supprime du stock les occurrences liées vers la denylist procédurale
    /// (purge one-shot ADR 0211). Une paire à la fois — chaque passe cible
    /// les blobs porteurs via le GIN `lj_cit_terms` (ADR 0247) et retranche
    /// les spans visés ; les blobs vidés sont supprimés en fin de purge.
    /// Renvoie le nombre de décisions réécrites.
    pub async fn delete_procedural_citations(
        &self,
        pairs: &HashSet<(String, String)>,
    ) -> Result<u64> {
        let mut total = 0u64;
        for (uid, key) in pairs {
            total += self
                .conn
                .execute(
                    "UPDATE legal_citation lc
                     SET spans = (
                         SELECT coalesce(jsonb_agg(s.el ORDER BY s.i), '[]'::jsonb)
                         FROM jsonb_array_elements(lc.spans) WITH ORDINALITY AS s(el, i)
                         WHERE s.el->>2 IS DISTINCT FROM $1
                            OR s.el->>3 IS DISTINCT FROM $2)
                     WHERE public.lj_cit_terms(lc.spans) @> ARRAY[$1 || '|' || $2]",
                    &[uid, key],
                )
                .await?;
        }
        self.conn
            .execute("DELETE FROM legal_citation WHERE spans = '[]'::jsonb", &[])
            .await?;
        Ok(total)
    }

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
                SELECT decision_id, spans
                FROM legal_citation
                WHERE decision_id = ANY($1) AND extract_version <= $2
                ",
                &[&decision_ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<OccurrenceFingerprint>> = HashMap::new();
        for row in &db_rows {
            let spans: serde_json::Value = row.get(1);
            let set = current.entry(row.get(0)).or_default();
            for el in spans.as_array().expect("spans : tableau jsonb (ADR 0247)") {
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
        let families = self
            .suivants_families(items.iter().flat_map(|(_, rows)| rows.iter()))
            .await?;
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> =
            Vec::with_capacity(items.len() * 3);
        let mut value_rows: Vec<String> = Vec::with_capacity(items.len());
        for (pos, (decision_id, rows)) in items.iter().enumerate() {
            let (instruments, composite) = citation_facet_arrays(rows, &families);
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

    /// Écrit la couche citations d'un lot de décisions (ADR 0145/0247) :
    /// DELETE des blobs existants, **COPY binaire** d'une ligne jsonb par
    /// décision porteuse dans `legal_citation`, puis
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

    /// Familles « et suivants » (ADR 0226) des occurrences liées porteuses du
    /// signal : une entrée `(uid, num_key) → num_keys` par ancre expansable —
    /// `_suivants_family_keys` (migrations 0149/0151, alphabet public partout)
    /// porte les garde-fous (section unique, VIGUEUR, cap 20 ; NULL sinon) et
    /// reste la source partagée avec les fonctions de resync. Zéro requête
    /// quand aucun span `suivants`.
    async fn suivants_families<'a>(
        &self,
        rows: impl Iterator<Item = &'a CitationOccurrenceRow>,
    ) -> Result<HashMap<(String, String), Vec<String>>> {
        let mut pairs: Vec<(String, String)> = rows
            .filter(|o| o.suivants)
            .filter_map(|o| Some((o.ref_text_uid.clone()?, o.ref_num_key.clone()?)))
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        if pairs.is_empty() {
            return Ok(HashMap::new());
        }
        let (uids, nums): (Vec<String>, Vec<String>) = pairs.into_iter().unzip();
        let rows = self
            .conn
            .query(
                "SELECT p.uid, p.num, _suivants_family_keys(p.uid, p.num) \
                 FROM unnest($1::text[], $2::text[]) AS p(uid, num)",
                &[&uids, &nums],
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let fam: Option<Vec<String>> = r.get(2);
                Some(((r.get(0), r.get(1)), fam?))
            })
            .collect())
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
                        SELECT substring(d.full_text FROM (el->>0)::int + 1
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
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, coalesce(t.title, ''), coalesce(t.title_key, ''),
                        coalesce(t.nature, ''), t.jurisdiction,
                        coalesce(t.num_prefix_agnostic, false),
                        count(a.*) FILTER (WHERE a.status = 'VIGUEUR'),
                        t.date_texte::text, t.nor
                 FROM legal_text t
                 LEFT JOIN legal_article a ON a.text_uid = t.text_uid
                 GROUP BY t.text_uid, t.title, t.title_key, t.nature, t.jurisdiction,
                          t.num_prefix_agnostic, t.date_texte, t.nor",
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

    /// Rebuild intégral de `citing_decision_counts` (ADR 0250) : décomptes de
    /// décisions citantes par terme cité (alphabet `lj_cit_terms` — `uid` ou
    /// `uid|num`). Une seule instruction (CTE modifiantes, même snapshot) :
    /// upsert des termes recomptés + suppression des termes disparus, garde
    /// `<>` pour ne réécrire que les décomptes changés. Hebdomadaire
    /// (`resync-legal-arrays`), ~2-4 min. Renvoie le nombre de termes upsertés.
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

    /// (Ré)écrit les renvois norme→article d'un TEXTE du référentiel
    /// (ADR 0217) : DELETE + COPY au grain `owner_text_uid` (corps + toutes
    /// ses versions d'articles d'un coup — la passe traite le texte entier).
    /// Idempotent.
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
        for r in rows {
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
                    &lj_core::EXTRACT_VERSION,
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
    }

    /// Spans de renvoi de la version d'article servie (ADR 0217), triés par
    /// `char_start`, joints au catalogue (slug/titre de la cible + gate
    /// « le texte a des articles » pour la mention nue).
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
                        EXISTS (SELECT 1 FROM legal_article a \
                                WHERE a.text_uid = lt.text_uid) AS has_articles \
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
                ref_has_articles: r.get(5),
            })
            .collect())
    }
}
