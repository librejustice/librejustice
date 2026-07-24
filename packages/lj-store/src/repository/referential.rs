//! Textes & articles de loi (`legal_text` / `legal_article`, ADR 0112) : upsert des
//! textes/articles, suppression LEGI par chemins, law-at-date, timeline des
//! versions, résolution de code, décisions citantes et sitemaps.
//!
//! ADR 0112 : l'identité d'un texte = `text_uid` (globalement unique, `source` hors
//! clé) ; l'identité d'une version d'article = `(text_uid, num_key, date_debut)`.
//! `source`/`source_uid`/`source_url` sont la PROVENANCE par version. Les citations
//! des décisions vivent dans `legal_citation` (module `citations`, ADR 0145),
//! liées au catalogue par le linker de la passe d'extraction.

use super::support::legal_article_row_from_row;
use super::types::{
    ArticleNeighborRow, ArticleRankHit, ArticleRrf, ArticleSearchRow, ArticleSearchStats,
    ArticleTitleMode, CitingDecisionRow, CoCitedArticleRow, FacetCount, FacetValueRow,
    JurisdictionRow, LawCodeSummaryRow, LawVersionRow, LegalArticleRow, LegalTextCatalogRow,
    LegalTextRow, SlugSourceRow, TocArticleRow, TocReadingRow,
};
use super::DecisionRepository;
use crate::error::Result;
use chrono::NaiveDate;
use lj_core::article_order::num_key_sort_key;
use tokio_postgres::types::ToSql;

/// Seuil de bascule des pages « décisions citantes » vers la marche par
/// récence (ADR 0250) : au-delà de ce nombre de décisions citantes
/// (`citing_decision_counts`, rebuild hebdo), le bitmap GIN + tri top-N
/// visite trop de postings ; la marche descendante de l'index date de
/// `decisions` trouve ses `limit` hits en ~`limit × N/df` itérations.
const CITING_RECENCY_WALK_MIN_COUNT: i64 = 100_000;

impl DecisionRepository<'_> {
    /// Lit le référentiel `facet_value` entier (ADR 0146) : labels FR,
    /// hiérarchie (`parent_uid`) et ordre d'affichage, dans l'ordre du seed
    /// (`facet, sort`). Alimente le cache référentiel in-process de `lj-api`.
    #[tracing::instrument(name = "db.load_facet_values", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_facet_values(&self) -> Result<Vec<FacetValueRow>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, facet, label, abbr, parent_uid, sort \
                 FROM facet_value ORDER BY facet, sort",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| FacetValueRow {
                uid: r.get(0),
                facet: r.get(1),
                label: r.get(2),
                abbr: r.get(3),
                parent_uid: r.get(4),
                sort: r.get(5),
            })
            .collect())
    }

    /// Lit le référentiel `jurisdiction` entier (ADR 0146) : une ligne par
    /// unité juridictionnelle (`tj_le_havre`, `ca_paris`…) avec code source,
    /// type, ville et label FR. Alimente le cache référentiel in-process de
    /// `lj-api` et les snapshots d'extraction (par `source_code`, ADR 0201).
    #[tracing::instrument(name = "db.load_jurisdictions", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_jurisdictions(&self) -> Result<Vec<JurisdictionRow>> {
        let rows = self
            .conn
            .query(
                "SELECT code, source_code, jurisdiction_type, city, label FROM jurisdiction",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| JurisdictionRow {
                code: r.get(0),
                source_code: r.get(1),
                jurisdiction_type: r.get(2),
                city: r.get(3),
                label: r.get(4),
            })
            .collect())
    }

    /// Upsert idempotent (#7) d'un texte de loi sur son identité `text_uid`. Réécrit
    /// le catalogue à chaque incrément ; rejoue sans dupliquer. `title_key` (=
    /// `normalize_instrument(title)`) est calculé par l'appelant et stocké tel quel.
    ///
    /// Ne touche jamais `slug` : un slug est immuable une fois posé (ADR 0162) et
    /// son unique écrivain est la passe [`Self::set_text_slugs`].
    ///
    /// Deux invariants inter-fonds (ADR 0225) :
    /// - `body` n'est jamais **effacé** par un upsert qui n'en apporte pas —
    ///   les corps viennent de passes dédiées (circulaires ADR 0222,
    ///   traités/TI ADR 0223) que les syncs de métadonnées ne doivent pas
    ///   balayer ;
    /// - une fiche `jurisdiction='INTL'` (traité, taggée par le fond JORF,
    ///   détection plus informée — ADR 0109) n'est pas rétrogradée par un
    ///   fond qui revoit le même CID en DECRET/LOI (des décrets de
    ///   publication vivent aussi en LEGI TNC).
    #[tracing::instrument(name = "db.upsert_legal_text", skip(self, text), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_text(&self, text: &LegalTextRow) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key,
                   body, status)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT (text_uid) DO UPDATE SET
                  jurisdiction = CASE WHEN legal_text.jurisdiction = 'INTL'
                                      THEN legal_text.jurisdiction
                                      ELSE EXCLUDED.jurisdiction END,
                  title = EXCLUDED.title,
                  title_key = EXCLUDED.title_key,
                  nature = CASE WHEN legal_text.jurisdiction = 'INTL'
                                THEN legal_text.nature
                                ELSE EXCLUDED.nature END,
                  last_modified = EXCLUDED.last_modified,
                  date_texte = EXCLUDED.date_texte,
                  date_publi = EXCLUDED.date_publi,
                  eli = EXCLUDED.eli,
                  nor = EXCLUDED.nor,
                  instrument_key = EXCLUDED.instrument_key,
                  body = COALESCE(EXCLUDED.body, legal_text.body),
                  status = EXCLUDED.status
                ",
                &[
                    &text.text_uid,
                    &text.jurisdiction,
                    &text.title,
                    &text.title_key,
                    &text.nature,
                    &text.last_modified,
                    &text.date_texte,
                    &text.date_publi,
                    &text.eli,
                    &text.nor,
                    &text.instrument_key,
                    &text.body,
                    &text.status,
                ],
            )
            .await?;
        Ok(())
    }

    /// Insère un texte **seulement s'il est absent** (`ON CONFLICT DO NOTHING`).
    /// Règle d'autorité de l'ingest JORF complet (ADR 0246, plan phase 4) : un
    /// JORFTEXT déjà porté (version consolidée LEGI/TNC, corps curé) n'est
    /// jamais écrasé par la fiche d'origine du fond JO. Renvoie `true` si la
    /// ligne a été créée.
    pub async fn insert_legal_text_if_absent(&self, text: &LegalTextRow) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key,
                   body, status)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT (text_uid) DO NOTHING
                ",
                &[
                    &text.text_uid,
                    &text.jurisdiction,
                    &text.title,
                    &text.title_key,
                    &text.nature,
                    &text.last_modified,
                    &text.date_texte,
                    &text.date_publi,
                    &text.eli,
                    &text.nor,
                    &text.instrument_key,
                    &text.body,
                    &text.status,
                ],
            )
            .await?;
        Ok(n == 1)
    }

    /// Pose le corps d'un texte de référentiel (passe corps circulaires,
    /// ADR 0222). UPDATE ciblé, jamais de création : `false` = `text_uid`
    /// inconnu (PDF orphelin du fond, compté par l'appelant).
    #[tracing::instrument(name = "db.set_legal_text_body", skip(self, body), fields(db.system = "postgresql"))]
    pub async fn set_legal_text_body(&self, text_uid: &str, body: &str) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_text SET body = $2 WHERE text_uid = $1",
                &[&text_uid, &body],
            )
            .await?;
        Ok(n > 0)
    }

    /// `text_uid` des textes d'une nature **sans articles ni corps réel**
    /// (`body` NULL ou < 300 caractères) — les cibles du backfill des corps
    /// traités/TI (ADR 0223).
    pub async fn empty_legal_text_uids(&self, nature: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text lt
                 WHERE nature = $1
                   AND (body IS NULL OR length(body) < 300)
                   AND NOT EXISTS (
                     SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid
                   )
                 ORDER BY text_uid",
                &[&nature],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Textes sans slug, `(text_uid, title)` triés par `text_uid` — l'ordre
    /// déterministe de la passe d'assignation (ADR 0162).
    pub async fn texts_without_slug(&self) -> Result<Vec<SlugSourceRow>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title, jurisdiction, date_texte::text, nor \
                 FROM legal_text WHERE slug IS NULL ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| SlugSourceRow {
                text_uid: r.get(0),
                title: r.get(1),
                jurisdiction: r.get(2),
                date_texte: r.get(3),
                nor: r.get(4),
            })
            .collect())
    }

    /// Slugs déjà posés (dédup de la passe d'assignation contre l'existant).
    pub async fn existing_text_slugs(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query("SELECT slug FROM legal_text WHERE slug IS NOT NULL", &[])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Pose les slugs calculés par la passe d'assignation (unique écrivain de la
    /// colonne, ADR 0162). Ne remplit que les `slug NULL` — un slug posé est
    /// immuable. Renvoie le nombre de lignes écrites.
    pub async fn set_text_slugs(&self, slugs: &[(String, String)]) -> Result<u64> {
        let uids: Vec<&str> = slugs.iter().map(|(u, _)| u.as_str()).collect();
        let vals: Vec<&str> = slugs.iter().map(|(_, s)| s.as_str()).collect();
        let n = self
            .conn
            .execute(
                "
                UPDATE legal_text t
                SET slug = v.slug
                FROM unnest($1::text[], $2::text[]) AS v(text_uid, slug)
                WHERE t.text_uid = v.text_uid AND t.slug IS NULL
                ",
                &[&uids, &vals],
            )
            .await?;
        Ok(n)
    }

    /// Pose le flag `num_prefix_agnostic` d'un texte (migration 0087) : la résolution
    /// d'article matchera sur le cœur numérique préfixe-strippé pour ce texte (codes
    /// territoriaux PF/NC cités avec un préfixe d'instrument incohérent). Séparé de
    /// `upsert_legal_text` (qui ne touche pas la colonne en `ON CONFLICT`) pour ne pas
    /// élargir `LegalTextRow` à tous ses sites de construction (LEGI/KALI/JORF…). Posé
    /// par le seul loader de corpus curé, qui garantit l'unicité du cœur par texte.
    pub async fn set_legal_text_num_prefix_agnostic(
        &self,
        text_uid: &str,
        flag: bool,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET num_prefix_agnostic = $2 WHERE text_uid = $1",
                &[&text_uid, &flag],
            )
            .await?;
        Ok(())
    }

    /// Upsert idempotent (#7) d'une version d'article sur son identité
    /// `(text_uid, num_key, date_debut)`. Skip si le `content_checksum` (xxh3-64 du
    /// bloc source brut) est inchangé : le `WHERE` sur `DO UPDATE` ne réécrit que les
    /// lignes dont le checksum diffère. Le checksum `u64` est stocké en `BIGINT` via
    /// cast bit-à-bit `i64::from_ne_bytes` (Postgres n'a pas d'u64). `date_debut`
    /// `None` → sentinelle '0001-01-01' (borne ouverte ; la PK interdit le NULL).
    ///
    /// **Garde-fou identité (ADR 0236)** : le `DO UPDATE` exige `source_uid` égal —
    /// un article chronique DISTINCT qui plie sur la même PK (numéro réellement
    /// dupliqué dans le texte, ex. « Annexe II » par livre) ne peut plus écraser
    /// silencieusement l'occupant. Le clash est détecté dans le même statement
    /// (l'occupant lu sur le snapshot pré-insert) et remonté en WARN — la ligne
    /// entrante est perdue, mais bruyamment (#12).
    /// Renvoie `true` si la ligne a été insérée ou modifiée, `false` si skip.
    #[tracing::instrument(name = "db.upsert_legal_article", skip(self, art), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_article(&self, art: &LegalArticleRow) -> Result<bool> {
        let checksum = i64::from_ne_bytes(art.content_checksum.to_ne_bytes());
        let row = self
            .conn
            .query_one(
                "
                WITH ins AS (
                  INSERT INTO legal_article
                    (text_uid, num, num_key, position, title_path, status, date_debut,
                     date_fin, texte, nota, content_checksum, source, source_uid, source_url,
                     texte_original, lang_original, translation, source_asof, source_upstream_url)
                  VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, DATE '0001-01-01'),
                          $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
                  ON CONFLICT (text_uid, num_key, date_debut) DO UPDATE SET
                    num = EXCLUDED.num,
                    position = EXCLUDED.position,
                    title_path = EXCLUDED.title_path,
                    status = EXCLUDED.status,
                    date_fin = EXCLUDED.date_fin,
                    texte = EXCLUDED.texte,
                    nota = EXCLUDED.nota,
                    content_checksum = EXCLUDED.content_checksum,
                    source = EXCLUDED.source,
                    source_uid = EXCLUDED.source_uid,
                    source_url = EXCLUDED.source_url,
                    texte_original = EXCLUDED.texte_original,
                    lang_original = EXCLUDED.lang_original,
                    translation = EXCLUDED.translation,
                    source_asof = EXCLUDED.source_asof,
                    source_upstream_url = EXCLUDED.source_upstream_url
                  WHERE legal_article.content_checksum IS DISTINCT FROM EXCLUDED.content_checksum
                    AND legal_article.source_uid = EXCLUDED.source_uid
                  RETURNING 1
                )
                SELECT (SELECT count(*) FROM ins)::int AS written,
                       (SELECT a.source_uid FROM legal_article a
                        WHERE a.text_uid = $1 AND a.num_key = $3
                          AND a.date_debut = COALESCE($7, DATE '0001-01-01')) AS holder
                ",
                &[
                    &art.text_uid,
                    &art.num,
                    &art.num_key,
                    &art.position,
                    &art.title_path,
                    &art.status,
                    &art.date_debut,
                    &art.date_fin,
                    &art.texte,
                    &art.nota,
                    &checksum,
                    &art.source,
                    &art.source_uid,
                    &art.source_url,
                    &art.texte_original,
                    &art.lang_original,
                    &art.translation,
                    &art.source_asof,
                    &art.source_upstream_url,
                ],
            )
            .await?;
        let written: i32 = row.get("written");
        let holder: Option<String> = row.get("holder");
        if written == 0 {
            if let Some(holder) = holder.filter(|h| *h != art.source_uid) {
                tracing::warn!(
                    text_uid = %art.text_uid,
                    num = %art.num,
                    num_key = %art.num_key,
                    date_debut = ?art.date_debut,
                    source = %art.source,
                    entrant = %art.source_uid,
                    occupant = %holder,
                    "CLASH D'IDENTITÉ D'ARTICLE (ADR 0236) : un article chronique distinct \
                     plie sur la même PK (text_uid, num_key, date_debut) — ligne entrante \
                     NON écrite, occupant conservé. À réviser : numéro réellement dupliqué \
                     dans le texte, ou clé d'identité à affiner."
                );
            }
        }
        Ok(written > 0)
    }

    /// Rafraîchit la fraîcheur « as-of » d'une source *live* re-synchronisée
    /// quotidiennement (legifrance/kali) : une ligne par source dans `ingest_freshness`,
    /// posée à chaque ingest (ADR 0129). Évite de réécrire les ~1,9 M lignes d'articles.
    #[tracing::instrument(name = "db.upsert_ingest_freshness", skip(self), fields(db.system = "postgresql"))]
    pub async fn upsert_ingest_freshness(&self, source: &str, asof: NaiveDate) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO ingest_freshness (source, asof) VALUES ($1, $2)
                 ON CONFLICT (source) DO UPDATE SET asof = EXCLUDED.asof",
                &[&source, &asof],
            )
            .await?;
        Ok(())
    }

    /// Fraîcheur « as-of » d'une source *live* (legifrance/kali) depuis
    /// `ingest_freshness` (ADR 0129). `None` si la source n'y figure pas (sources
    /// non-live : la fraîcheur vit alors sur `legal_article.source_asof`).
    #[tracing::instrument(name = "db.get_ingest_freshness", skip(self), fields(db.system = "postgresql"))]
    pub async fn get_ingest_freshness(&self, source: &str) -> Result<Option<NaiveDate>> {
        let row = self
            .conn
            .query_opt(
                "SELECT asof FROM ingest_freshness WHERE source = $1",
                &[&source],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Paires distinctes `(source, source_url)` des lignes dont `source` est une
    /// **catégorie/méthode** à reclasser (ADR 0131 : `treaty`, `eu-law`, `official-fr`,
    /// `traduction-automatique`) **et** qui portent une URL — d'où le vrai libellé de
    /// diffuseur se dérive (`diffuseur_label_from_url`). Lecture seule.
    #[tracing::instrument(name = "db.distinct_category_source_urls", skip(self), fields(db.system = "postgresql"))]
    pub async fn distinct_category_source_urls(
        &self,
        sources: &[&str],
    ) -> Result<Vec<(String, String)>> {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT source, source_url FROM legal_article \
                 WHERE source = ANY($1) AND COALESCE(source_url, '') <> ''",
                &[&sources],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Reclasse `source` (ADR 0131) pour les lignes d'un couple `(ancien source, URL)`
    /// donné vers le `nouveau` libellé de diffuseur. Renvoie le nombre de lignes. Idempotent.
    #[tracing::instrument(name = "db.relabel_source_by_url", skip(self), fields(db.system = "postgresql"))]
    pub async fn relabel_source_by_url(&self, old: &str, url: &str, new: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = $3 WHERE source = $1 AND source_url = $2",
                &[&old, &url, &new],
            )
            .await?;
        Ok(n)
    }

    /// Reclasse les traités du **bulk JORF** (`source='treaty'`, sans URL, `source_uid`
    /// natif `JORFARTI…`) en `source='jorf'` (diffuseur DILA, ADR 0131) — la nature
    /// « traité » reste portée par `jurisdiction='INTL'`. Exclut les **chaînes curées**
    /// (avenants, `source_uid` propre au dataset) traitées par reload. Renvoie le nombre
    /// de lignes. Idempotent.
    #[tracing::instrument(name = "db.relabel_treaty_jorf_bulk", skip(self), fields(db.system = "postgresql"))]
    pub async fn relabel_treaty_jorf_bulk(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = 'jorf' \
                 WHERE source = 'treaty' AND COALESCE(source_url, '') = '' \
                   AND source_uid LIKE 'JORFARTI%'",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Canonicalise un libellé de diffuseur (`legal_article.source`) : `from` → `to`,
    /// pour les variantes d'hôte d'un même diffuseur (ex. `jafbase.fr` → `jafbase`,
    /// ADR 0131 « un libellé par diffuseur »). Idempotent (après coup plus aucune ligne
    /// `from`). Renvoie le nombre de lignes modifiées.
    #[tracing::instrument(name = "db.canonicalize_source_label", skip(self), fields(db.system = "postgresql"))]
    pub async fn canonicalize_source_label(&self, from: &str, to: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = $2 WHERE source = $1",
                &[&from, &to],
            )
            .await?;
        Ok(n)
    }

    /// Reliquat post-reclassement : lignes encore sous un `source` catégorie/méthode
    /// (URL vide, hors bulk JORF) — chaînes curées de traités & vieux codes traduits,
    /// à corriger par reload de curation. `(source, nombre)`. Lecture seule.
    #[tracing::instrument(name = "db.count_category_source_leftovers", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_category_source_leftovers(
        &self,
        sources: &[&str],
    ) -> Result<Vec<(String, i64)>> {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let rows = self
            .conn
            .query(
                "SELECT source, count(*) FROM legal_article \
                 WHERE source = ANY($1) GROUP BY source ORDER BY source",
                &[&sources],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// `text_uid` (= `JORFTEXT…`/`EU/…`) de tous les textes internationaux du
    /// catalogue (`jurisdiction ∈ ('INTL','UE')` = traités & droit primaire UE ;
    /// le clivage traité ↔ JORF se lit sur la juridiction, pas sur `source`). Lu entre
    /// les deux passes de l'ingest JORF (textes puis articles) pour ne **persister**
    /// que les articles dont le `cid` parent est un traité (le reste du JO est ignoré) ;
    /// leur `source` reste `jorf` (diffuseur DILA, ADR 0131). Lecture seule, idempotent.
    #[tracing::instrument(name = "db.treaty_text_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn treaty_text_uids(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text WHERE jurisdiction IN ('INTL', 'UE')",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Suppression d'articles d'un fond DILA par chemins issus des
    /// `liste_suppression_<fond>*.dat` (canal séparé, ADR 0092/0120). Le stem du
    /// chemin `.dat` (`…/LEGIARTI000….xml` / `…/KALIARTI000….xml` ⇒ l'id) est le
    /// `source_uid` (identifiant natif provider, hors identité) à purger pour le
    /// `source` donné (`'legifrance'` LEGI/JORF, `'kali'` KALI). Renvoie le nombre de
    /// lignes supprimées. Idempotent : un `source_uid` déjà absent ne compte pas.
    #[tracing::instrument(name = "db.delete_legal_articles_by_paths", skip(self, paths), fields(db.system = "postgresql", source, paths = paths.len()))]
    pub async fn delete_legal_articles_by_paths(
        &self,
        source: &str,
        paths: &[String],
    ) -> Result<u64> {
        if paths.is_empty() {
            return Ok(0);
        }
        let source_uids: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                let stem = p.rsplit('/').next().unwrap_or(p);
                let id = stem.strip_suffix(".xml").unwrap_or(stem);
                if id.is_empty() {
                    None
                } else {
                    Some(id.to_string())
                }
            })
            .collect();
        if source_uids.is_empty() {
            return Ok(0);
        }
        // Purge liée des arêtes du graphe (ADR 0174) AVANT les articles : la clé
        // owner de `legal_link` se résout via la ligne d'article encore présente.
        self.conn
            .execute(
                "DELETE FROM legal_link ll USING legal_article a \
                 WHERE a.source = $1 AND a.source_uid = ANY($2) \
                   AND ll.owner_text_uid = a.text_uid AND ll.owner_num_key = a.num_key \
                   AND ll.owner_date_debut = a.date_debut",
                &[&source, &source_uids],
            )
            .await?;
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE source = $1 AND source_uid = ANY($2)",
                &[&source, &source_uids],
            )
            .await?;
        Ok(n)
    }

    /// Pose l'état de diffusion d'un lot de textes (ADR 0196 — abrogations
    /// historiques du fond CIRCULAIRES). Ne touche que la `nature` donnée
    /// (jamais un uid d'une autre famille par collision). Renvoie le nombre de
    /// lignes modifiées ; un uid absent est ignoré (les listes d'abrogation
    /// référencent des documents jamais publiés dans les stocks).
    #[tracing::instrument(name = "db.set_legal_texts_status", skip(self, text_uids), fields(db.system = "postgresql", nature, status, uids = text_uids.len()))]
    pub async fn set_legal_texts_status(
        &self,
        nature: &str,
        text_uids: &[String],
        status: &str,
    ) -> Result<u64> {
        if text_uids.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "UPDATE legal_text SET status = $3 \
                 WHERE nature = $1 AND text_uid = ANY($2)",
                &[&nature, &text_uids, &status],
            )
            .await?;
        Ok(n)
    }

    /// Corps monolithiques du référentiel (ADR 0196) : `(text_uid, body)` des
    /// textes à corps — source de l'extraction texte→décision.
    pub async fn legal_text_bodies(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, body FROM legal_text WHERE body IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Page keyset de TOUTES les versions d'articles à corps (ADR 0217) —
    /// émetteurs de la passe renvois/cases. Ordonnée par la PK
    /// `(text_uid, num_key, date_debut)` : les versions d'un même texte
    /// arrivent groupées, le writer peut flush au changement de `text_uid`.
    /// `after` = dernière clé de la page précédente (`("", "", 0001-01-01)`
    /// pour la première).
    pub async fn legal_article_versions_page(
        &self,
        after: (&str, &str, NaiveDate),
        limit: i64,
    ) -> Result<Vec<(String, String, NaiveDate, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num_key, date_debut, texte \
                 FROM legal_article \
                 WHERE (text_uid, num_key, date_debut) > ($1, $2, $3) \
                   AND texte IS NOT NULL \
                 ORDER BY text_uid, num_key, date_debut \
                 LIMIT $4",
                &[&after.0, &after.1, &after.2, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Purge des articles d'un texte hors snapshot courant (source snapshot type
    /// BOFiP, ADR 0196) : supprime les lignes du `source`/`text_uid` dont la version
    /// n'est pas `keep_date` OU dont le `num_key` a disparu du document. Rejouable :
    /// un snapshot identique ne supprime rien. Les arêtes `legal_link` des lignes
    /// purgées partent d'abord (même règle que [`Self::delete_legal_articles_by_paths`]).
    #[tracing::instrument(name = "db.delete_legal_articles_versions_except", skip(self, keep_num_keys), fields(db.system = "postgresql", source, text_uid))]
    pub async fn delete_legal_articles_versions_except(
        &self,
        source: &str,
        text_uid: &str,
        keep_date: chrono::NaiveDate,
        keep_num_keys: &[String],
    ) -> Result<u64> {
        self.conn
            .execute(
                "DELETE FROM legal_link ll USING legal_article a \
                 WHERE a.source = $1 AND a.text_uid = $2 \
                   AND (a.date_debut <> $3 OR a.num_key <> ALL($4)) \
                   AND ll.owner_text_uid = a.text_uid AND ll.owner_num_key = a.num_key \
                   AND ll.owner_date_debut = a.date_debut",
                &[&source, &text_uid, &keep_date, &keep_num_keys],
            )
            .await?;
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE source = $1 AND text_uid = $2 \
                   AND (date_debut <> $3 OR num_key <> ALL($4))",
                &[&source, &text_uid, &keep_date, &keep_num_keys],
            )
            .await?;
        Ok(n)
    }

    /// Purge tous les articles d'un texte donné (`text_uid`, globalement unique).
    /// Sert le loader de corpus curé (`load-legal-corpus`) : un dataset est
    /// autoritaire pour son texte, on purge avant rechargement (idempotence #7). Vaut
    /// pour la forme mono-version (un article par `num_key`) comme multi-versions
    /// (plusieurs lignes par `num_key`, identité `(text_uid, num_key, date_debut)`).
    /// Renvoie le nombre de lignes supprimées.
    #[tracing::instrument(name = "db.delete_legal_articles_by_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_legal_articles_by_text(&self, text_uid: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        Ok(n)
    }

    /// Collapse des doublons diffuseur (ADR 0115 §2) : supprime les coquilles
    /// `LEGITEXT` **vides** (0 article) dont un **autre** texte porte le corps sous le
    /// même `title_key` (le `JORFTEXT` porteur — LEGI keye ses lois non codifiées sous
    /// des ids JORFTEXT). L'identité d'un acte = le porteur de corps, pas la coquille.
    /// Idempotent, sûr : ne touche jamais un texte **cité** (`ref_text_uid`) ni un
    /// texte porteur d'articles. Recrée-able par l'upsert si un corps arrive plus tard.
    /// À lancer en fin d'ingest LEGI (les coquilles régénérées par incrément). Renvoie
    /// le nombre de coquilles collapsées.
    #[tracing::instrument(name = "db.collapse_empty_legitext_doublons", skip(self), fields(db.system = "postgresql"))]
    pub async fn collapse_empty_legitext_doublons(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                DELETE FROM legal_text t
                WHERE t.text_uid LIKE 'LEGITEXT%'
                  AND t.jurisdiction = 'FR'
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                  AND EXISTS (
                        SELECT 1 FROM legal_text b
                        JOIN legal_article ba ON ba.text_uid = b.text_uid
                        WHERE b.text_uid <> t.text_uid
                          AND lower(b.title_key) = lower(t.title_key))
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_citation c
                        WHERE public.lj_cit_terms(c.spans) @> ARRAY[t.text_uid])
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Rôles des textes publiés (ADR 0246), backfill v1 conservateur : reset à
    /// `instrument` puis classification par signaux sûrs — motifs de titre pour
    /// `individuel` et `habilitation`, et pour `vehicule` un décret « portant
    /// publication » vide (ni corps ni article) porteur d'une arête `modifie`
    /// sortante résolue (avenant publié dont l'instrument vit sur la fiche de
    /// base — le décret de publication *principal* d'un traité n'a pas cette
    /// arête et reste `instrument`, avec ou sans corps). Idempotent, recalcule
    /// tout. Renvoie (individuel, habilitation, vehicule).
    #[tracing::instrument(name = "db.backfill_text_roles", skip(self), fields(db.system = "postgresql"))]
    pub async fn backfill_text_roles(&self) -> Result<(u64, u64, u64)> {
        self.conn
            .execute(
                "UPDATE legal_text SET role = 'instrument' WHERE role <> 'instrument'",
                &[],
            )
            .await?;
        // Motifs mesurés sur le fond JORF complet (sondes 2026-07-21, faux
        // positifs vérifiés sur échantillon) : « portant radiation » nu est
        // ambigu (radiation de produits/spécialités des listes = normatif) —
        // seuls « radiation des cadres » et « radiation (corps) » classent.
        let individuel = self
            .conn
            .execute(
                "
                UPDATE legal_text SET role = 'individuel'
                WHERE text_uid LIKE 'JORFTEXT%'
                  AND (title ~* 'portant (nomination|promotion|titularisation|naturalisation|cessation de fonctions|admission à la retraite|acceptation de la démission|détachement|radiation des cadres|radiation \\()'
                       OR title ~* '^avis de vacance'
                       OR title ~* 'accordant la nationalité française|conférant l.honorariat|acceptant la démission|inscription au tableau d.avancement')
                ",
                &[],
            )
            .await?;
        let habilitation = self
            .conn
            .execute(
                "
                UPDATE legal_text SET role = 'habilitation'
                WHERE text_uid LIKE 'JORFTEXT%'
                  AND title ~* 'autorisant (la ratification|l.approbation|l.adhésion|l.accession|le Président de la République à (ratifier|approuver|adhérer))'
                  AND title !~* ' et (portant|modifiant)'
                ",
                &[],
            )
            .await?;
        let vehicule = self
            .conn
            .execute(
                "
                UPDATE legal_text t SET role = 'vehicule'
                WHERE t.text_uid LIKE 'JORFTEXT%'
                  AND t.title ~* 'portant publication'
                  AND t.body IS NULL
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                  AND EXISTS (
                        SELECT 1 FROM legal_link l
                        WHERE l.owner_text_uid = t.text_uid
                          AND l.verb = 'modifie'
                          AND l.direction = 'outgoing'
                          AND l.target_text_uid IS NOT NULL
                          AND l.target_text_uid <> t.text_uid)
                ",
                &[],
            )
            .await?;
        Ok((individuel, habilitation, vehicule))
    }

    /// Aligne `legal_link.verb` sur le repli verbe/nom courant de
    /// `lj_extract::legi::lien_verb` pour le stock écrit avant l'extension du
    /// mapping (ADR 0246 §2). Idempotent. Renvoie le nombre de lignes alignées.
    #[tracing::instrument(name = "db.normalize_link_verbs", skip(self), fields(db.system = "postgresql"))]
    pub async fn normalize_link_verbs(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                UPDATE legal_link SET verb = m.verb
                FROM (VALUES
                        ('RATIFIE', 'ratifie'), ('RATIFICATION', 'ratifie'),
                        ('DENONCE', 'denonce'), ('DENONCIATION', 'denonce'),
                        ('ANNULE', 'annule'), ('ANNULATION', 'annule'),
                        ('DISJOINT', 'disjoint'), ('DISJONCTION', 'disjoint'),
                        ('ETEND', 'etend'), ('EXTENSION', 'etend'),
                        ('RECTIFIE', 'rectifie'), ('TRANSPOSITION', 'transpose')
                     ) AS m (typelien, verb)
                WHERE legal_link.typelien = m.typelien
                  AND legal_link.verb <> m.verb
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Article servi à une date (law-at-date, ADR 0112 §7). `date` absente ⇒ version
    /// en vigueur (`status = 'VIGUEUR'`). Sinon, la version dont l'intervalle
    /// `[date_debut, date_fin]` couvre `date` (la plus récente en cas de
    /// chevauchement) ; `date_debut` est NOT NULL (sentinelle '0001-01-01' = borne
    /// ouverte basse). `None` si aucune version ne s'applique. Résolution vers
    /// l'IDENTITÉ `(text_uid, num_key)`, la version choisie ici par la date.
    #[tracing::instrument(name = "db.law_article_at_date", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_at_date(
        &self,
        text_uid: &str,
        num_key: &str,
        date: Option<NaiveDate>,
    ) -> Result<Option<LegalArticleRow>> {
        let row = match date {
            Some(d) => {
                self.conn
                    .query_opt(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM legal_article
                        WHERE text_uid = $1 AND num_key = $2
                          AND date_debut <= $3
                          AND (date_fin IS NULL OR date_fin >= $3)
                        ORDER BY date_debut DESC
                        LIMIT 1
                        ",
                        &[&text_uid, &num_key, &d],
                    )
                    .await?
            }
            None => {
                // VIGUEUR préférée, sinon la dernière version (ADR 0162 §5) :
                // un article abrogé reste lisible, son état est affiché.
                self.conn
                    .query_opt(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM legal_article
                        WHERE text_uid = $1 AND num_key = $2
                        ORDER BY (status = 'VIGUEUR') DESC, date_debut DESC NULLS LAST
                        LIMIT 1
                        ",
                        &[&text_uid, &num_key],
                    )
                    .await?
            }
        };
        Ok(row.map(legal_article_row_from_row))
    }

    /// Lecture « à la suite » façon Légifrance (ADR 0112 §9) : les articles du texte
    /// `text_uid`, version en vigueur à `date` (ou `VIGUEUR` si `date` absente), triés
    /// par `position` (ordre de lecture réel, ≠ tri lexical). Un deep-link sur un
    /// article = pré-scroll sur sa `position`, l'utilisateur scrolle sur les voisins
    /// (26, 26-1, 26-2…). `position` NULL (référentiel pré-0112 pas encore réingéré)
    /// trié en dernier ; fallback `num_key`. Champs bruts (mapping DTO côté `lj-api`).
    #[tracing::instrument(name = "db.law_text_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_text_articles(
        &self,
        text_uid: &str,
        date: Option<NaiveDate>,
    ) -> Result<Vec<LegalArticleRow>> {
        // Une version par num_key : la plus récente couvrant `date` (ou la VIGUEUR).
        // DISTINCT ON (num_key) + ORDER BY garantit une ligne par article ; on retrie
        // ensuite par position pour la lecture.
        let rows = match date {
            Some(d) => {
                self.conn
                    .query(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM (
                          SELECT DISTINCT ON (num_key) *
                          FROM legal_article
                          WHERE text_uid = $1 AND date_debut <= $2
                            AND (date_fin IS NULL OR date_fin >= $2)
                          ORDER BY num_key, date_debut DESC
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        ",
                        &[&text_uid, &d],
                    )
                    .await?
            }
            None => {
                self.conn
                    .query(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM (
                          -- VIGUEUR préférée, sinon dernière version (ADR 0162 §5).
                          SELECT DISTINCT ON (num_key) *
                          FROM legal_article
                          WHERE text_uid = $1
                          ORDER BY num_key, (status = 'VIGUEUR') DESC,
                                   date_debut DESC NULLS LAST
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        ",
                        &[&text_uid],
                    )
                    .await?
            }
        };
        Ok(rows.into_iter().map(legal_article_row_from_row).collect())
    }

    /// Timeline des versions d'un article (ADR 0112), triée par `date_debut`
    /// croissante. `source_uid` = identifiant natif de la version (LEGIARTI…). Dates
    /// rendues en ISO `String` (`::text`) ; le mapping DTO est fait côté `lj-api`.
    #[tracing::instrument(name = "db.law_article_versions", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_versions(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Vec<LawVersionRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT source_uid, status, date_debut::text, date_fin::text
                FROM legal_article
                WHERE text_uid = $1 AND num_key = $2
                ORDER BY date_debut
                ",
                &[&text_uid, &num_key],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| LawVersionRow {
                source_uid: r.get(0),
                status: r.get(1),
                date_debut: r.get(2),
                date_fin: r.get(3),
            })
            .collect())
    }

    /// Sommaire d'un texte par son `slug` (ADR 0112) : métadonnées du `legal_text` +
    /// nombre d'articles distincts toutes versions (ADR 0162 §5). `None` si le slug
    /// est inconnu (pas de fallback #12 — l'appelant rend un 404).
    #[tracing::instrument(name = "db.law_code_summary", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_code_summary(&self, slug: &str) -> Result<Option<LawCodeSummaryRow>> {
        let row = self
            .conn
            .query_opt(
                "
                SELECT t.text_uid, t.slug, t.title, t.nature, t.last_modified::text,
                       -- Toutes versions confondues (ADR 0162 §5) : un texte
                       -- abrogé affiche ses articles, pas un sommaire vide.
                       (SELECT count(DISTINCT a.num_key)
                        FROM legal_article a
                        WHERE a.text_uid = t.text_uid) AS article_count,
                       t.upcoming_versions,
                       t.body, t.status, t.nor, t.date_texte::text
                FROM legal_text t
                WHERE t.slug = $1
                ",
                &[&slug],
            )
            .await?;
        Ok(row.map(|r| LawCodeSummaryRow {
            text_uid: r.get(0),
            slug: r.get(1),
            title: r.get(2),
            nature: r.get(3),
            last_modified: r.get(4),
            article_count: r.get(5),
            upcoming_versions: r.get(6),
            body: r.get(7),
            status: r.get(8),
            nor: r.get(9),
            date_texte: r.get(10),
        }))
    }

    /// Pose les dates de versions futures d'un texte (ADR 0178) : colonne
    /// hors `LegalTextRow` (un seul écrivain, l'ingest LEGI — même patron que
    /// `num_prefix_agnostic`). Écrit toujours, y compris vide (un incrément
    /// qui vide `VERSIONS_A_VENIR` vide la colonne).
    pub async fn set_legal_text_upcoming_versions(
        &self,
        text_uid: &str,
        dates: &[NaiveDate],
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET upcoming_versions = $2 WHERE text_uid = $1",
                &[&text_uid, &dates],
            )
            .await?;
        Ok(())
    }

    /// Résout un slug de code en `text_uid` par lookup **exact** (ADR 0112 §6 /
    /// ADR 0123 §2).
    ///
    /// `slug` = la chaîne d'URL `/texte/{slug}` — un slug canonique que **nos liens
    /// portent** (stocké au DTO). Plus de `normalize_instrument` ni de fallback BM25
    /// au runtime serve : c'est ce qui sort la pile `lj-extract` du chemin serve.
    /// `None` si le slug est inconnu → 404 côté appelant (#12), jamais de pick
    /// silencieux ; une forme legacy/tapée-main perd le rattrapage flou (assumé).
    #[tracing::instrument(name = "db.resolve_referential_code", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_referential_code(&self, slug: &str) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT text_uid FROM legal_text WHERE slug = $1 LIMIT 1",
                &[&slug],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Résout un slug d'instrument MCP (forme nom-libre slugifiée par
    /// l'appelant) vers son `text_uid`, alphabet des colonnes de filtre.
    /// `None` = inconnu → erreur corrective côté MCP, jamais de filtre
    /// silencieusement vide (#12).
    #[tracing::instrument(name = "db.resolve_instrument_uid", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_instrument_uid(&self, slug: &str) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt("SELECT text_uid FROM legal_text WHERE slug = $1", &[&slug])
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Le slug existe-t-il au catalogue ? — pour router `get_legal_text`
    /// vers `/texte/{slug}` après slugification du nom libre.
    #[tracing::instrument(name = "db.law_slug_exists", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_slug_exists(&self, slug: &str) -> Result<bool> {
        let row = self
            .conn
            .query_opt("SELECT 1 FROM legal_text WHERE slug = $1", &[&slug])
            .await?;
        Ok(row.is_some())
    }

    /// Suggestions `(slug, titre)` pour une valeur d'instrument inconnue, par
    /// similarité trigramme tolérante aux fautes (pg_trgm, migration 0140).
    /// `similarity` plein-champ (slug et titre entiers, pas `word_similarity`) :
    /// un extent interne ne score plus — « code-civile » suggère « code-civil »
    /// avant les lois longues dont le titre contient « code civil ».
    #[tracing::instrument(name = "db.suggest_instruments", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_instruments(
        &self,
        needle: &str,
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT slug, title FROM legal_text \
                 WHERE slug IS NOT NULL \
                 AND GREATEST(similarity($1, slug), similarity($1, title)) > 0.4 \
                 ORDER BY GREATEST(similarity($1, slug), similarity($1, title)) DESC, \
                 length(title), title LIMIT $2",
                &[&needle, &limit],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Titre humain d'un texte de référentiel par son `slug` — pour l'affichage
    /// « Article N du <titre> » (le slug est une clé d'URL, pas un libellé). `None`
    /// si le slug est inconnu.
    #[tracing::instrument(name = "db.referential_title", skip(self), fields(db.system = "postgresql"))]
    pub async fn referential_title(&self, slug: &str) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT title FROM legal_text WHERE slug = $1 LIMIT 1",
                &[&slug],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Décisions citant un article (ADR 0112 §2 / 0145 M4) : backlinks depuis
    /// les blobs `legal_citation.spans` (GIN `lj_cit_terms`, ADR 0247).
    /// Paginée sous cap de fenêtre (`offset + limit ≤ 100`, tenu par la
    /// validation d'entrée — routes). Champs bruts (mapping DTO côté `lj-api`).
    ///
    /// Deux plans selon le volume de citantes (`citing_decision_counts`,
    /// ADR 0250) :
    /// - article courant : bitmap GIN, tri portée (gabarit 
    ///   « arrêt majeur », ADR 0167) puis date décroissante ;
    /// - article ultra-cité (≥ [`CITING_RECENCY_WALK_MIN_COUNT`]) : marche
    ///   descendante de `idx_decisions_date_lecture` avec filtre
    ///   d'appartenance sur le blob — récence seule (trier par portée
    ///   exigerait de visiter tous les postings).
    ///
    /// Bornée à la fenêtre de validité `[date_debut, date_fin)` de la version
    /// servie : une décision rendue à la date D cite la version en vigueur à D,
    /// pas une autre (`num_key` seul est version-agnostique — cf. renumérotations,
    /// ex. cautionnement 2288 refondu au 2022-01-01). `date_fin` `None` = version
    /// en vigueur (borne haute ouverte).
    #[tracing::instrument(name = "db.law_decisions_citing", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_decisions_citing(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        date_debut: NaiveDate,
        date_fin: Option<NaiveDate>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CitingDecisionRow>> {
        let citing_count: i64 = self
            .conn
            .query_opt(
                "SELECT decision_count FROM citing_decision_counts \
                 WHERE cited_term = $1 || '|' || $2",
                &[&ref_text_uid, &num_key],
            )
            .await?
            .map(|r| r.get(0))
            .unwrap_or(0);
        let params: [&(dyn ToSql + Sync); 6] = [
            &ref_text_uid,
            &num_key,
            &limit,
            &offset,
            &date_debut,
            &date_fin,
        ];
        let rows = if citing_count >= CITING_RECENCY_WALK_MIN_COUNT {
            // Marche par récence : l'EXISTS sur le blob n'est PAS indexable
            // (pas de `@>` sur l'expression GIN) — le planner ne peut que
            // descendre l'index date, coût ≈ limit × N/df itérations.
            // `d.date_lecture >= $5` exclut les NULL : le scan arrière de
            // l'index date sert l'ORDER BY sans tri.
            self.conn
                .query(
                    "
                    SELECT d.id, d.public_id, d.jurisdiction_type,
                           j.label, d.date_lecture::text, d.docket_numbers,
                           d.publication_codes, d.summary
                    FROM decisions d
                    JOIN legal_citation lc ON lc.decision_id = d.id
                    LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                    WHERE d.date_lecture >= $5
                      AND ($6::date IS NULL OR d.date_lecture < $6)
                      AND EXISTS (
                          SELECT 1 FROM jsonb_array_elements(lc.spans) AS el
                          WHERE el->>2 = $1 AND el->>3 = $2)
                    ORDER BY d.date_lecture DESC, d.id
                    LIMIT $3 OFFSET $4
                    ",
                    &params,
                )
                .await?
        } else {
            // Autorité d'abord (gabarit  « arrêt majeur ») : rang de
            // portée dérivé de `publication_codes` (mêmes groupes que la
            // facette, ADR 0167/`lj_core::publication`), puis date
            // décroissante. Les listes de codes sont des constantes du code —
            // inlinées, pas des données.
            let rank_expr = {
                let arr = |group: &str| {
                    let codes: Vec<String> = lj_core::publication::significance_codes(group)
                        .iter()
                        .map(|c| format!("'{c}'"))
                        .collect();
                    format!("ARRAY[{}]", codes.join(","))
                };
                format!(
                    "CASE WHEN d.publication_codes && {maj} THEN 0 \
                          WHEN d.publication_codes && {imp} THEN 1 \
                          WHEN d.publication_codes && {lim} THEN 2 \
                          ELSE 3 END",
                    maj = arr("majeure"),
                    imp = arr("importante"),
                    lim = arr("limitee"),
                )
            };
            self.conn
                .query(
                    &format!(
                        "
                        SELECT d.id, d.public_id, d.jurisdiction_type,
                               j.label, d.date_lecture::text, d.docket_numbers,
                               d.publication_codes, d.summary,
                               {rank_expr} AS significance_rank
                        FROM legal_citation lc
                        JOIN decisions d ON d.id = lc.decision_id
                        LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                        WHERE public.lj_cit_terms(lc.spans) @> ARRAY[$1 || '|' || $2]
                          -- fenêtre de validité [date_debut, date_fin) de la version
                          -- servie ; date_fin NULL = en vigueur (borne haute ouverte).
                          AND d.date_lecture >= $5
                          AND ($6::date IS NULL OR d.date_lecture < $6)
                        -- Texte ISO 'YYYY-MM-DD' → tri == chronologique. Une ligne
                        -- blob par décision : pas de doublons à dédupliquer.
                        ORDER BY significance_rank, d.date_lecture::text DESC NULLS LAST, d.id
                        LIMIT $3 OFFSET $4
                        "
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
                date_lecture: r.get(4),
                docket_numbers: r.get(5),
                publication_codes: r.get(6),
                summary: r.get(7),
            })
            .collect())
    }

    /// Fil d'Ariane TOC d'une version d'article : les divisions enclosantes,
    /// de la racine à la section directe (`label`, `child_cid`). Marche
    /// remontante depuis l'arête de la version servie ; en cas d'arêtes
    /// multiples au même niveau (section LEGI ré-écrite), une seule chaîne est
    /// retenue. Vide si la version n'est pas dans la TOC (JORF, étranger).
    #[tracing::instrument(name = "db.article_toc_breadcrumb", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_toc_breadcrumb(
        &self,
        text_uid: &str,
        article_uid: &str,
    ) -> Result<Vec<(String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "
                WITH RECURSIVE anchor AS (
                    SELECT e.owner_uid FROM legal_toc_edge e
                    WHERE e.text_uid = $1 AND e.child_kind = 'article'
                      AND e.child_uid = $2
                    ORDER BY e.seq LIMIT 1
                ), up AS (
                    SELECT e.owner_uid, e.label, e.child_cid, 0 AS d
                    FROM legal_toc_edge e
                    JOIN anchor a ON e.child_uid = a.owner_uid
                    UNION ALL
                    SELECT e.owner_uid, e.label, e.child_cid, up.d + 1
                    FROM legal_toc_edge e
                    JOIN up ON e.child_uid = up.owner_uid
                    WHERE up.d < 12
                )
                SELECT DISTINCT ON (d) label, child_cid FROM up
                ORDER BY d DESC
                ",
                &[&text_uid, &article_uid],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Articles co-cités avec `(ref_text_uid, num_key)` dans les décisions
    /// (« souvent cité avec », plan graphe Phase D) : échantillon des décisions
    /// citant l'article (500, borne le coût sur les articles ultra-cités), puis
    /// agrégat de leurs autres citations pondéré tf-idf saturé (ADR 0250) —
    /// score `n/(n+20) × ln(N / df)` où `df` vient de
    /// `citing_decision_counts` (rebuild hebdo) et `N` de l'estimation
    /// planner de `decisions`. La saturation BM25 du tf est nécessaire :
    /// en IDF linéaire, 700 CPC (n ≈ 490/500, IDF ≈ 1,2) dominait encore
    /// l'article doctrinal (n ≈ 47, IDF ≈ 5) — mesuré sur civil 1240,
    /// 2026-07-21. Seuil ≥ 3 co-occurrences ; le compte AFFICHÉ reste brut.
    #[tracing::instrument(name = "db.law_co_cited_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_co_cited_articles(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        limit: i64,
    ) -> Result<Vec<CoCitedArticleRow>> {
        let rows = self
            .conn
            .query(
                "
                WITH citing AS (
                    SELECT decision_id FROM legal_citation
                    WHERE public.lj_cit_terms(spans) @> ARRAY[$1 || '|' || $2]
                    LIMIT 500
                ), co AS (
                    SELECT el->>2 AS ref_text_uid, el->>3 AS ref_num_key,
                           count(DISTINCT lc.decision_id) AS n
                    FROM legal_citation lc
                    JOIN citing c ON c.decision_id = lc.decision_id
                    CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
                    WHERE el->>3 IS NOT NULL
                      AND (el->>2 <> $1 OR el->>3 <> $2)
                    GROUP BY 1, 2
                    HAVING count(DISTINCT lc.decision_id) >= 3
                )
                SELECT co.ref_num_key, co.n, t.slug, t.title
                FROM co
                JOIN legal_text t ON t.text_uid = co.ref_text_uid
                -- df absent (terme jamais recompté — article tout neuf) :
                -- repli df = n_co, le plus conservateur des connus.
                LEFT JOIN citing_decision_counts s
                       ON s.cited_term = co.ref_text_uid || '|' || co.ref_num_key
                ORDER BY (co.n::float8 / (co.n + 20)) * ln(
                             (SELECT greatest(reltuples, 1)::float8 FROM pg_class
                              WHERE oid = 'decisions'::regclass)
                             / greatest(coalesce(s.decision_count, co.n), 1)::float8
                         ) DESC,
                         co.ref_num_key
                LIMIT $3
                ",
                &[&ref_text_uid, &num_key, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CoCitedArticleRow {
                num_key: r.get(0),
                count: r.get(1),
                text_slug: r.get(2),
                text_title: r.get(3),
            })
            .collect())
    }

    /// Itère `(slug, num, lastmod)` pour les articles en vigueur (sitemaps
    /// `/texte/{slug}/{num}`, ADR 0112). `lastmod` = `COALESCE(t.last_modified,
    /// a.date_debut, '1970-01-01')`, capé à `current_date` : DILA pose la
    /// sentinelle 2999-01-01 (« vigueur indéfinie ») dans ces dates, et un
    /// lastmod futur est invalide pour Google. Ordre déterministe `(slug, num)` ;
    /// pas de pagination SQL — `build_sitemaps` pagine en mémoire.
    #[tracing::instrument(name = "db.iter_referential_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_referential_for_sitemap(&self) -> Result<Vec<(String, String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT t.slug, a.num_key,
                       LEAST(
                           COALESCE(t.last_modified, a.date_debut, DATE '1970-01-01'),
                           current_date
                       )::date AS lastmod
                FROM legal_article a
                JOIN legal_text t ON t.text_uid = a.text_uid
                WHERE a.status = 'VIGUEUR' AND t.slug IS NOT NULL
                ORDER BY t.slug, a.num_key
                ",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Itère `(slug, lastmod)` pour les codes navigables (pages TDM
    /// `/texte/{slug}`, ADR 0237). Même filtre que `list_legal_texts` (le
    /// catalogue `/codes`) : `slug` non nul, nature *navigable-comme-un-code*,
    /// ≥1 article en vigueur. `lastmod` = `t.last_modified` capé à
    /// `current_date` (DILA pose la sentinelle 2999). Ordre déterministe par
    /// slug ; `build_sitemaps` pagine en mémoire.
    #[tracing::instrument(name = "db.iter_codes_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_codes_for_sitemap(&self) -> Result<Vec<(String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT t.slug,
                       LEAST(COALESCE(t.last_modified, current_date), current_date)::date
                           AS lastmod
                FROM legal_text t
                WHERE t.slug IS NOT NULL
                  AND (t.nature ILIKE 'code%'
                       OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI',
                           'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT',
                           'ETAT_CIVIL'))
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid AND a.status = 'VIGUEUR')
                ORDER BY t.slug
                ",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Rafraîchit les colonnes dénormalisées de `legal_article` depuis
    /// `legal_text` pour les articles dont elles diffèrent : `code_title`
    /// (titre du code parent, ADR 0114) et les colonnes de recherche
    /// `jurisdiction`/`nature` (upper)/`slug`/`searchable` (ADR 0254). Appelé
    /// en fin d'ingest référentiel : LEGI streame articles et codes
    /// séparément, l'article n'a pas les attributs de son texte au parse → on
    /// les pose ici. La colonne générée `search_title` (titre formé indexé)
    /// est recalculée par Postgres sur les lignes touchées. Renvoie le nombre
    /// de lignes mises à jour. Idempotent (#7).
    #[tracing::instrument(name = "db.refresh_article_denorm", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_article_denorm(&self) -> Result<u64> {
        // `UPDATE` global sur tout `legal_article` (~M lignes) : le scan du join
        // dépasse le `statement_timeout` du pool (30 s) dès que le corpus grossit
        // (observé sur load-legal-corpus). On le lève **localement**, dans une
        // transaction dédiée — l'UPDATE n'écrit que les lignes dont une colonne
        // a dérivé, reste idempotent (#7).
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let n = self
                .conn
                .execute(
                    "UPDATE legal_article a SET \
                       code_title = t.title, \
                       jurisdiction = t.jurisdiction, \
                       nature = upper(t.nature), \
                       slug = t.slug, \
                       searchable = (t.slug IS NOT NULL \
                         AND t.role NOT IN ('individuel', 'vehicule', 'habilitation')) \
                     FROM legal_text t \
                     WHERE a.text_uid = t.text_uid \
                       AND (a.code_title IS DISTINCT FROM t.title \
                         OR a.jurisdiction IS DISTINCT FROM t.jurisdiction \
                         OR a.nature IS DISTINCT FROM upper(t.nature) \
                         OR a.slug IS DISTINCT FROM t.slug \
                         OR a.searchable IS DISTINCT FROM (t.slug IS NOT NULL \
                           AND t.role NOT IN ('individuel', 'vehicule', 'habilitation')))",
                    &[],
                )
                .await?;
            Ok(n)
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

    /// Recherche plein-texte d'articles (ADR 0114/0232/0233,
    /// `/recherche-textes`), **titre-primaire**. Par jambe, le prédicat
    /// combine titre conjonctif normalisé ×4 + filet titre OR ×0,25 + corps
    /// ([`article_search_predicates`]) ; la fusion entre les jambes se fait
    /// **par rang** (RRF, `1/(k + rang)`, jambes bornées à
    /// [`ARTICLE_LEG_LIMIT`]) — les scores BM25 des index ne sont pas
    /// comparables, les rangs le sont (ADR 0232, mesuré : nDCG@10 0,44 vs
    /// 0,09). Cinq jambes (ADR 0234 puis 0235, nDCG@10 0,50 vs 0,38) :
    /// - articles **domestiques** (jurisdiction FR/UE/INTL + pays nommés dans
    ///   la requête, [`lj_core::jurisdictions::query_jurisdictions`]) ;
    /// - articles **étrangers**, pondérés [`ARTICLE_FOREIGN_WEIGHT`] — les
    ///   codes napoléoniens étrangers matchent mot pour mot les requêtes
    ///   françaises et trustaient le top ;
    /// - articles du **pays nommé** (ADR 0238,
    ///   [`lj_core::jurisdictions::strip_query_jurisdictions`]) : requête
    ///   débarrassée des tokens pays, bornée aux juridictions nommées — les
    ///   articles de fond étrangers ne contiennent pas le nom de leur pays ;
    ///   la jambe domestique exclut alors ces juridictions ;
    /// - « textes à corps » (ADR 0196) ;
    /// - **conteneurs** : textes navigables comme un code (filtre nature du
    ///   catalogue, ADR 0133) sans corps mais à articles, titre matché par la
    ///   forme conjonctive de la requête ou d'une expansion d'alias **pleine
    ///   requête** (ADR 0238) — un hit `num = ''` (lien `/texte/{slug}`),
    ///   prioritaire à rang égal (requête navigationnelle « code de la
    ///   famille sénégalais », « code civil du sénégal »).
    /// Articles `VIGUEUR`, optionnellement bornés à un `text_uid`.
    /// `slug`/`code_title` joints pour le lien `/texte/{slug}/{num}` ;
    /// `score` = score RRF fusionné.
    #[tracing::instrument(name = "db.search_articles", skip(self, expansions), fields(db.system = "postgresql", limit, offset))]
    #[allow(clippy::too_many_arguments)]
    pub async fn search_articles(
        &self,
        query: &str,
        expansions: &[String],
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
        nature_set: Option<(&[String], bool)>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ArticleSearchRow>> {
        // `$1` titre OR (requête enrichie), `$2` corps (requête brute ; la
        // clause usage_terms est ajoutée boostée dans
        // [`article_search_predicates`], ADR 0248), alternatives conjonctives en
        // `$3..`, puis filtres optionnels et pagination.
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let (art_pred, txt_pred) = article_search_predicates(&mut params, query, expansions);
        let (art_filters, txt_filters) =
            params.push_filters(text_uid, jurisdiction, nature, source, nature_set);
        let prim_ph = params.push(Box::new(primary_jurisdictions(query)));
        // Jambe « pays nommé » (ADR 0238) : articles des juridictions que la
        // requête nomme, requête débarrassée des tokens pays — les articles
        // de fond étrangers ne contiennent pas le nom de leur pays, le BM25
        // favorise les docs qui le portent (conventions fiscales…). La jambe
        // articles domestique exclut alors ces juridictions (pas de doublon).
        let (art_ctry_excl, ctry_cte, ctry_union) =
            match lj_core::jurisdictions::strip_query_jurisdictions(query) {
                Some((codes, stripped)) => {
                    let codes: Vec<String> = codes.into_iter().map(String::from).collect();
                    let codes_ph = params.push(Box::new(codes));
                    let conj_clause = match lj_core::aliases::conj_title_query(&stripped) {
                        Some(c) => {
                            let cph = params.push(Box::new(c));
                            format!(
                                "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                                 paradedb.match('search_title', ${cph}, \
                                 conjunction_mode => true)), "
                            )
                        }
                        None => String::new(),
                    };
                    let s_ph = params.push(Box::new(stripped));
                    (
                        format!("\n  AND a.jurisdiction <> ALL(${codes_ph})"),
                        // Single-table (ADR 0254) : prédicat 100 % indexé, les
                        // colonnes d'affichage (dont slug/code_title
                        // dénormalisés) jointes pour le seul top-K.
                        format!(
                            "
                    ctry AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.boolean(should => ARRAY[{conj_clause}\
                                paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                                paradedb.match('texte', ${s_ph})])
                            AND a.status = 'VIGUEUR'
                            AND a.searchable
                            AND a.jurisdiction = ANY(${codes_ph}){art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                      ) x
                    ),"
                        ),
                        format!(
                            "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -1 AS leg
                      FROM ctry"
                        ),
                    )
                }
                None => (String::new(), String::new(), String::new()),
            };
        // Jambe conteneurs : forme conjonctive de la requête + expansions
        // d'alias PLEINE requête (« code civil du sénégal » → « code de la
        // famille sénégalais », ADR 0238 — jamais les expansions embarquées,
        // ADR 0234).
        let mut cont_alts: Vec<String> = Vec::new();
        cont_alts.extend(lj_core::aliases::conj_title_query(query));
        cont_alts.extend(
            lj_core::aliases::whole_query_expansions(query)
                .iter()
                .filter_map(|e| lj_core::aliases::conj_title_query(e)),
        );
        cont_alts.dedup();
        let (cont_cte, cont_union) = match cont_alts.as_slice() {
            [_, ..] => {
                let clauses: Vec<String> = cont_alts
                    .iter()
                    .map(|alt| {
                        let ph = params.push(Box::new(alt.clone()));
                        format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                    })
                    .collect();
                let cont_match = match clauses.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                };
                (
                    // Fence `OFFSET 0` : seuls le match titre et la juridiction
                    // (fast fields) descendent dans le scan ParadeDB — les
                    // filtres non indexables (`nature ILIKE`…) redescendus en
                    // heap filter forçaient un parcours de toute `legal_text`
                    // (~1,6 s pour souvent 0 ligne).
                    format!(
                        "
                    cont AS (
                      SELECT z.*, row_number() OVER (ORDER BY z.score DESC, z.slug) AS rk
                      FROM (
                        SELECT t.text_uid, t.slug, t.title,
                               coalesce(t.status, 'VIGUEUR') AS status,
                               lower(t.nature) AS source,
                               t.score
                        FROM (
                          SELECT t.text_uid, t.slug, t.title, t.status, t.nature,
                                 t.role, t.jurisdiction, (t.body IS NULL) AS no_body,
                                 paradedb.score(t.id) AS score
                          FROM legal_text t
                          WHERE t.id @@@ {cont_match}
                            AND t.jurisdiction = ANY(${prim_ph})
                          OFFSET 0
                        ) t
                        WHERE t.no_body
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                          AND EXISTS (SELECT 1 FROM legal_article a
                                      WHERE a.text_uid = t.text_uid
                                        AND a.status = 'VIGUEUR'){txt_filters}
                        ORDER BY t.score DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) z
                    ),"
                    ),
                    format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, '' AS num, '' AS num_key,
                             NULL AS title_path, status, source,
                             NULL::text AS texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -2 AS leg
                      FROM cont"
                    ),
                )
            }
            [] => (String::new(), String::new()),
        };
        // Jambe « usage » (ADR 0248) : grammes de la requête contre les sacs
        // de contextes de citation (`legal_article.usage_terms`). Elle RECOUVRE
        // la jambe articles : ses votes se SOMMENT (GROUP BY final) —
        // accumulation d'évidence, jamais un simple interleaving. Coupée sur
        // les requêtes-référence et navigationnelles (garde).
        let (us_cte, us_union) = if lj_core::usage::usage_reference_or_nav_query(query) {
            (String::new(), String::new())
        } else {
            let g_ph = params.push(Box::new(lj_core::usage::usage_grams(query)));
            (
                // Le filtre navigable + visible et l'affichage (slug,
                // code_title) viennent des colonnes dénormalisées de l'article
                // résolu par le LATERAL (ADR 0254) — plus de join legal_text.
                format!(
                    "
                    us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.texte, paradedb.score(u.id) AS score
                        FROM legal_article_usage u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND a2.status = 'VIGUEUR'
                          ORDER BY a2.date_debut DESC LIMIT 1
                        ) a ON true
                        WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          AND a.searchable{art_filters}
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                ),
                format!(
                    "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, texte,
                             {ARTICLE_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 3 AS leg
                      FROM us"
                ),
            )
        };
        let limit_ph = params.push(Box::new(limit));
        let offset_ph = params.push(Box::new(offset));
        // Jambes articles domestique et étrangère : deux scans ParadeDB
        // **single-table TopK** (ADR 0254) — prédicat et filtres 100 % indexés
        // (searchable/jurisdiction dénormalisés) + `ORDER BY score LIMIT`
        // directement dans le scan, donc élagage WAND de bout en bout, y
        // compris sur le boolean+boost (mesuré : 110 ms + 87 ms contre
        // 1,05 s pour un scan partagé sans coupe qui score les ~415 k docs
        // du filet corps). Les colonnes d'affichage (texte, title_path,
        // slug/code_title dénormalisés…) ne sont jointes que pour les
        // ≤ leg_limit lignes retenues. La coupe du top-K se fait par score
        // seul (le tiebreak `num_key` du `row_number` n'intervient qu'après
        // la coupe), puis fusion par rang. Tiebreak final par `leg` :
        // conteneur, pays nommé, articles domestiques, textes à corps,
        // articles étrangers. La pagination porte sur la fusion
        // (≤ 5 × leg_limit docs atteignables — sans effet : le front pagine
        // par 10-20, le MCP plafonne limit à 20).
        let sql = format!(
            "
                    WITH{cont_cte}{ctry_cte}{us_cte}
                    art AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND a.status = 'VIGUEUR'
                            AND a.searchable
                            AND a.jurisdiction = ANY(${prim_ph}){art_ctry_excl}{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                      ) x
                    ),
                    art_f AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND a.status = 'VIGUEUR'
                            AND a.searchable
                            AND NOT (a.jurisdiction = ANY(${prim_ph})){art_ctry_excl}{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                      ) x
                    ),
                    txt AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.text_uid, t.slug, t.title,
                               coalesce(t.status, 'VIGUEUR') AS status,
                               lower(t.nature) AS source, t.body AS texte,
                               paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE {txt_pred}
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL{txt_filters}
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) y
                    )
                    SELECT u.text_uid, u.slug, u.title, u.num, u.num_key,
                           u.title_path, u.status, u.source, u.texte,
                           sum(u.rrf)::float4 AS score
                    FROM (
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, 0 AS leg
                      FROM art
                      UNION ALL
                      SELECT text_uid, slug, title, '' AS num, '' AS num_key,
                             NULL AS title_path, status, source, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, 1 AS leg
                      FROM txt
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, texte,
                             {ARTICLE_FOREIGN_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 2 AS leg
                      FROM art_f{ctry_union}{cont_union}{us_union}
                    ) u
                    GROUP BY u.text_uid, u.slug, u.title, u.num, u.num_key,
                             u.title_path, u.status, u.source, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key
                    LIMIT ${limit_ph} OFFSET ${offset_ph}
                    "
        );
        let rows = self.conn.query(&sql, &params.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| ArticleSearchRow {
                text_uid: r.get(0),
                slug: r.get(1),
                code_title: r.get(2),
                num: r.get(3),
                num_key: r.get(4),
                title_path: r.get(5),
                status: r.get(6),
                source: r.get(7),
                texte: r.get(8),
                score: r.get(9),
            })
            .collect())
    }

    /// Total exact + les quatre facettes de la recherche d'articles (ADR 0114) en
    /// **une** requête `GROUPING SETS`, sous le même prédicat BM25 + filtres que
    /// [`Self::search_articles`] : le prédicat (le poste dominant, ~1 s sur le
    /// corpus complet) ne s'exécute qu'une fois au lieu de quatre. Les comptes
    /// restent ceux du prédicat complet (filtres inclus) : l'UI montre la
    /// composition du résultat courant, pas un univers non filtré. Chaque axe est
    /// trié count décroissant puis valeur ascendante ; les valeurs vides sont
    /// écartées. `nature` est normalisée `upper()` (le corpus curé mélange
    /// `LOI`/`loi`, `CONSTITUTION`/`constitution`).
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(name = "db.article_search_stats", skip(self, expansions), fields(db.system = "postgresql"))]
    pub async fn article_search_stats(
        &self,
        query: &str,
        expansions: &[String],
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
        nature_set: Option<(&[String], bool)>,
    ) -> Result<ArticleSearchStats> {
        // Corps = requête brute ; la clause usage_terms (ADR 0248) est ajoutée
        // par `article_search_predicates`, donc facettes cohérentes avec les
        // résultats de `search_articles`.
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let (art_pred, txt_pred) = article_search_predicates(&mut params, query, expansions);
        let (art_filters, txt_filters) =
            params.push_filters(text_uid, jurisdiction, nature, source, nature_set);
        // Membre conteneurs (ADR 0234) : même appartenance que la jambe de
        // hits — le prior de juridiction ne change pas l'appartenance des
        // jambes articles (pondération de fusion seulement), il n'apparaît
        // donc pas ici, sauf pour les conteneurs où c'est un filtre dur.
        let mut cont_alts: Vec<String> = Vec::new();
        cont_alts.extend(lj_core::aliases::conj_title_query(query));
        cont_alts.extend(
            lj_core::aliases::whole_query_expansions(query)
                .iter()
                .filter_map(|e| lj_core::aliases::conj_title_query(e)),
        );
        cont_alts.dedup();
        let cont_member = match cont_alts.as_slice() {
            [_, ..] => {
                let clauses: Vec<String> = cont_alts
                    .iter()
                    .map(|alt| {
                        let ph = params.push(Box::new(alt.clone()));
                        format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                    })
                    .collect();
                let cont_match = match clauses.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                };
                let prim_ph = params.push(Box::new(primary_jurisdictions(query)));
                // Même fence `OFFSET 0` que la jambe conteneurs des hits :
                // seuls le match titre et la juridiction descendent dans le
                // scan, les filtres non indexables restent côté SQL.
                format!(
                    "
                      UNION ALL
                      SELECT t.slug, t.jurisdiction, upper(t.nature) AS nature,
                             lower(t.nature) AS source
                      FROM (
                        SELECT t.text_uid, t.slug, t.jurisdiction, t.nature,
                               t.status, t.role, (t.body IS NULL) AS no_body
                        FROM legal_text t
                        WHERE t.id @@@ {cont_match}
                          AND t.jurisdiction = ANY(${prim_ph})
                        OFFSET 0
                      ) t
                      WHERE t.no_body
                        AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                        AND t.slug IS NOT NULL
                        AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                        AND EXISTS (SELECT 1 FROM legal_article a
                                    WHERE a.text_uid = t.text_uid
                                      AND a.status = 'VIGUEUR'){txt_filters}"
                )
            }
            [] => String::new(),
        };
        // `GROUPING(...)` encode le set actif en bitmask (bit levé = colonne NON
        // groupée) : code = 0b0111, jurisdiction = 0b1011, nature = 0b1101,
        // source = 0b1110, total (grand agrégat) = 0b1111. Mêmes jambes que
        // la page de hits (les boosts n'affectent pas l'appartenance : comptes
        // cohérents avec les hits sans dépendre de la fusion RRF).
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT GROUPING(u.slug, u.jurisdiction, u.nature, u.source) AS gset,
                           u.slug, u.jurisdiction, u.nature, u.source, count(*) AS n
                    FROM (
                      SELECT a.slug, a.jurisdiction, a.nature, a.source
                      FROM legal_article a
                      WHERE {art_pred}
                        AND a.status = 'VIGUEUR'
                        AND a.searchable{art_filters}
                      UNION ALL
                      SELECT t.slug, t.jurisdiction, upper(t.nature) AS nature,
                             lower(t.nature) AS source
                      FROM legal_text t
                      WHERE {txt_pred}
                        AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                        AND t.slug IS NOT NULL{txt_filters}{cont_member}
                    ) u
                    GROUP BY GROUPING SETS ((u.slug), (u.jurisdiction), (u.nature), (u.source), ())
                    "
                ),
                &params.refs(),
            )
            .await?;
        let mut stats = ArticleSearchStats {
            total: 0,
            code: Vec::new(),
            jurisdiction: Vec::new(),
            nature: Vec::new(),
            source: Vec::new(),
        };
        for r in &rows {
            let gset: i32 = r.get(0);
            let count: i64 = r.get(5);
            let (axis, value): (&mut Vec<FacetCount>, Option<String>) = match gset {
                0b0111 => (&mut stats.code, r.get(1)),
                0b1011 => (&mut stats.jurisdiction, r.get(2)),
                0b1101 => (&mut stats.nature, r.get(3)),
                0b1110 => (&mut stats.source, r.get(4)),
                _ => {
                    stats.total = count;
                    continue;
                }
            };
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                axis.push(FacetCount { value, count });
            }
        }
        for axis in [
            &mut stats.code,
            &mut stats.jurisdiction,
            &mut stats.nature,
            &mut stats.source,
        ] {
            axis.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        }
        Ok(stats)
    }

    /// Bras du banc de ranking articles (`lj-bench article-rank-eval`) : les
    /// variantes du prédicat titre ([`ArticleTitleMode`]) et de la fusion
    /// inter-jambes (score brut vs RRF) de [`Self::search_articles`], sans
    /// filtre ni pagination. `(TitleMode::Or, rrf = None)` est la baseline
    /// pré-ADR 0232 (OR ×4 + tri par score brut) ; la prod actuelle
    /// correspond à `(OrConj, or_boost 0,25, rrf k=60, container,
    /// foreign_weight 0,25)` (ADR 0234).
    #[tracing::instrument(name = "db.search_articles_rank_arm", skip(self, expansions), fields(db.system = "postgresql", limit))]
    pub async fn search_articles_rank_arm(
        &self,
        query: &str,
        expansions: &[String],
        title_mode: ArticleTitleMode,
        or_boost: f64,
        rrf: Option<ArticleRrf>,
        limit: i64,
    ) -> Result<Vec<ArticleRankHit>> {
        // Jambe titre selon le mode : `Or` = requête enrichie ($1, comme la
        // prod) boostée ×4 ; `Conj` = un `should` d'alternatives TOUTES
        // conjonctives — requête et expansions d'alias, chacune normalisée
        // pour le titre ([`lj_core::aliases::conj_title_query`] : « article »
        // éliminé, nums recollés) et devant matcher en entier — boosté ×4 ;
        // `OrConj` = les deux clauses, conjonctif ×4 + OR ×`or_boost` (le
        // conjonctif prime quand il matche, l'OR reste un filet pour les
        // requêtes que la conjonction rejette). Rien ne subsiste après
        // normalisation → repli Or.
        // Postgres ne type pas un placeholder jamais référencé : $1 porte donc
        // la forme utile au mode choisi.
        let mut conj_alts: Vec<String> = std::iter::once(query.to_string())
            .chain(expansions.iter().cloned())
            .filter_map(|q| lj_core::aliases::conj_title_query(&q))
            .collect();
        conj_alts.dedup();
        let title_mode = if conj_alts.is_empty() {
            ArticleTitleMode::Or
        } else {
            title_mode
        };
        let title_param = match title_mode {
            ArticleTitleMode::Conj => conj_alts[0].clone(),
            _ => article_title_query(query, expansions),
        };
        let mut params = ArticleSearchParams::new(title_param, query);
        let mut alt_phs: Vec<usize> = Vec::new();
        match title_mode {
            ArticleTitleMode::Or => {}
            ArticleTitleMode::Conj => {
                alt_phs.push(1);
                for alt in &conj_alts[1..] {
                    alt_phs.push(params.push(Box::new(alt.clone())));
                }
            }
            ArticleTitleMode::OrConj => {
                for alt in &conj_alts {
                    alt_phs.push(params.push(Box::new(alt.clone())));
                }
            }
        }
        let title_clause = |field: &str| {
            let or = format!("paradedb.match('{field}', $1)");
            let conj = || {
                let alts: Vec<String> = alt_phs
                    .iter()
                    .map(|ph| format!("paradedb.match('{field}', ${ph}, conjunction_mode => true)"))
                    .collect();
                match alts.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                }
            };
            match title_mode {
                ArticleTitleMode::Or => format!("paradedb.boost(4, {or})"),
                ArticleTitleMode::Conj => format!("paradedb.boost(4, {})", conj()),
                // `or_boost` module le filet OR (valeur code-contrôlée,
                // inlinée : paradedb.boost n'accepte pas de placeholder).
                ArticleTitleMode::OrConj => {
                    format!(
                        "paradedb.boost(4, {}), paradedb.boost({or_boost}, {or})",
                        conj()
                    )
                }
            }
        };
        let art_pred = format!(
            "a.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('texte', $2)]) \
             AND {TEXT_ROLE_VISIBLE_SQL}",
            title_clause("search_title")
        );
        let txt_pred = format!(
            "t.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('body', $2)]) \
             AND t.body IS NOT NULL AND {TEXT_ROLE_VISIBLE_SQL}",
            title_clause("title")
        );

        let sql = match rrf {
            None => {
                let limit_ph = params.push(Box::new(limit));
                format!(
                    "
                    SELECT * FROM (
                      SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                             paradedb.score(a.id) AS score
                      FROM legal_article a
                      JOIN legal_text t ON t.text_uid = a.text_uid
                      WHERE {art_pred}
                        AND a.status = 'VIGUEUR'
                        AND t.slug IS NOT NULL
                      UNION ALL
                      SELECT t.slug, '' AS num, '' AS num_key, t.title,
                             NULL AS title_path, t.body AS texte,
                             paradedb.score(t.id) AS score
                      FROM legal_text t
                      WHERE {txt_pred}
                        AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                        AND t.slug IS NOT NULL
                    ) u
                    ORDER BY u.score DESC, u.num_key
                    LIMIT ${limit_ph}
                    "
                )
            }
            Some(ArticleRrf {
                k,
                txt_weight,
                leg_limit,
                split_title: true,
                ..
            }) => {
                let k_ph = params.push(Box::new(k));
                let w_ph = params.push(Box::new(txt_weight));
                let leg_ph = params.push(Box::new(leg_limit));
                let limit_ph = params.push(Box::new(limit));
                // 4 jambes mono-clause (titre et corps séparés, comme la
                // recherche décisions) : un doc fort dans UNE jambe surface,
                // le bruit d'une jambe ne contamine pas l'ordre des autres.
                // Scores RRF sommés par doc (un article peut sortir des deux
                // jambes articles). `$1` = requête titre enrichie (mode Or).
                format!(
                    "
                    WITH art_t AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                               paradedb.score(a.id) AS score
                        FROM legal_article a
                        JOIN legal_text t ON t.text_uid = a.text_uid
                        WHERE a.id @@@ paradedb.match('search_title', $1)
                          AND a.status = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(a.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),
                    art_b AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                               paradedb.score(a.id) AS score
                        FROM legal_article a
                        JOIN legal_text t ON t.text_uid = a.text_uid
                        WHERE a.id @@@ paradedb.match('texte', $2)
                          AND a.status = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(a.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),
                    txt_t AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, t.body AS texte, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ paradedb.match('title', $1)
                          AND t.body IS NOT NULL
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) y
                    ),
                    txt_b AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, t.body AS texte, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ paradedb.match('body', $2)
                          AND t.body IS NOT NULL
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) y
                    )
                    SELECT u.slug, u.num, u.title, u.title_path, u.texte FROM (
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 0 AS leg
                      FROM art_t
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 1 AS leg
                      FROM art_b
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, texte,
                             ${w_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 2 AS leg
                      FROM txt_t
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, texte,
                             ${w_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 3 AS leg
                      FROM txt_b
                    ) u
                    GROUP BY u.slug, u.num, u.num_key, u.title, u.title_path, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key
                    LIMIT ${limit_ph}
                    "
                )
            }
            Some(ArticleRrf {
                k,
                txt_weight,
                leg_limit,
                split_title: false,
                container,
                foreign_weight,
                container_alias,
                country_leg,
                foreign_score_merge,
                usage_weight,
                usage_table,
            }) => {
                let k_ph = params.push(Box::new(k));
                let w_ph = params.push(Box::new(txt_weight));
                let leg_ph = params.push(Box::new(leg_limit));
                let limit_ph = params.push(Box::new(limit));
                // Prior de juridiction optionnel : la jambe articles se scinde
                // en domestique (FR/UE/INTL + pays nommés dans la requête) et
                // étrangère pondérée `foreign_weight` — les codes napoléoniens
                // étrangers matchent mot pour mot les requêtes françaises.
                // Deux fusions : par rang (jambe RRF séparée, prod) ou par
                // score (`foreign_score_merge`, membre UNION pondéré dans la
                // jambe articles — équivalent boost Tantivy, même index donc
                // scores comparables).
                let mut prim_ph: Option<usize> = None;
                let (art_juris, art_merge_union, artf_cte, artf_union) = match foreign_weight {
                    Some(w) if foreign_score_merge => {
                        let ph = params.push(Box::new(primary_jurisdictions(query)));
                        prim_ph = Some(ph);
                        (
                            format!("\n  AND t.jurisdiction = ANY(${ph})"),
                            format!(
                                "
                        UNION ALL
                        (SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                               paradedb.score(a.id) * {w}::float8 AS score
                        FROM legal_article a
                        JOIN legal_text t ON t.text_uid = a.text_uid
                        WHERE {art_pred}
                          AND a.status = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND t.jurisdiction <> ALL(${ph})
                        ORDER BY paradedb.score(a.id) DESC
                        LIMIT ${leg_ph})"
                            ),
                            String::new(),
                            String::new(),
                        )
                    }
                    Some(w) => {
                        let ph = params.push(Box::new(primary_jurisdictions(query)));
                        prim_ph = Some(ph);
                        (
                            format!("\n  AND t.jurisdiction = ANY(${ph})"),
                            String::new(),
                            format!(
                                "
                    art_f AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                               paradedb.score(a.id) AS score
                        FROM legal_article a
                        JOIN legal_text t ON t.text_uid = a.text_uid
                        WHERE {art_pred}
                          AND a.status = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND t.jurisdiction <> ALL(${ph})
                        ORDER BY paradedb.score(a.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             {w}::float8 / (${k_ph}::float8 + rk) AS rrf, 2 AS leg
                      FROM art_f"
                            ),
                        )
                    }
                    None => (String::new(), String::new(), String::new(), String::new()),
                };
                // Jambe « termes d'usage » optionnelle (working-note
                // 2026-07-20) : requête en grammes contre les sacs de
                // contextes de citation, jointe à la version en vigueur.
                // Coupée sur les requêtes-référence et navigationnelles.
                let usage_weight =
                    usage_weight.filter(|_| !lj_core::usage::usage_reference_or_nav_query(query));
                let (usage_cte, usage_union) = match usage_weight {
                    Some(uw) => {
                        let gq = lj_core::usage::usage_grams(query);
                        let g_ph = params.push(Box::new(gq));
                        let uw_ph = params.push(Box::new(uw));
                        let usage_table = usage_table.unwrap_or("legal_article_usage");
                        (
                            format!(
                                "
                    us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT t.slug, la.num, u.num_key, t.title, la.title_path, la.texte,
                               paradedb.score(u.id) AS score
                        FROM {usage_table} u
                        JOIN legal_text t ON t.text_uid = u.text_uid
                        JOIN LATERAL (
                          SELECT a.num, a.title_path, a.texte FROM legal_article a
                          WHERE a.text_uid = u.text_uid AND a.num_key = u.num_key
                            AND a.status = 'VIGUEUR'
                          ORDER BY a.date_debut DESC LIMIT 1
                        ) la ON true
                        WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             ${uw_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 3 AS leg
                      FROM us"
                            ),
                        )
                    }
                    None => (String::new(), String::new()),
                };
                // Fusion finale : les jambes historiques sont doc-disjointes
                // (scission par juridiction, types de docs différents) — le
                // merge-sort des votes suffit. La jambe usage RECOUVRE la
                // jambe articles : ses votes doivent se SOMMER par doc
                // (accumulation d'évidence, comme la branche `split_title`),
                // sinon elle ne peut qu'injecter des docs, jamais renforcer
                // un doc déjà trouvé.
                let final_order = if usage_weight.is_some() {
                    "GROUP BY u.slug, u.num, u.num_key, u.title, u.title_path, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key"
                } else {
                    "ORDER BY u.rrf DESC, u.leg, u.num_key"
                };
                // Jambe « pays nommé » optionnelle (ADR 0238) : articles des
                // juridictions que la requête nomme, requête débarrassée des
                // tokens pays — « conditions du divorce au sénégal » : les
                // articles de fond ne contiennent pas « sénégal », le BM25
                // favorise les docs qui le portent (conventions fiscales…).
                // La jambe articles exclut alors ces juridictions (pas de
                // doublon inter-jambes). Tiebreak -1 : sous le conteneur,
                // au-dessus du domestique.
                let ctry = country_leg
                    .then(|| lj_core::jurisdictions::strip_query_jurisdictions(query))
                    .flatten();
                let (art_ctry_excl, ctry_cte, ctry_union) = match ctry {
                    Some((codes, stripped)) => {
                        let codes: Vec<String> = codes.into_iter().map(String::from).collect();
                        let codes_ph = params.push(Box::new(codes));
                        let conj_clause = match lj_core::aliases::conj_title_query(&stripped) {
                            Some(c) => {
                                let cph = params.push(Box::new(c));
                                format!(
                                    "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                                         paradedb.match('search_title', ${cph}, \
                                         conjunction_mode => true)), "
                                )
                            }
                            None => String::new(),
                        };
                        let s_ph = params.push(Box::new(stripped));
                        (
                            format!("\n  AND t.jurisdiction <> ALL(${codes_ph})"),
                            format!(
                                "
                    ctry AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                               paradedb.score(a.id) AS score
                        FROM legal_article a
                        JOIN legal_text t ON t.text_uid = a.text_uid
                        WHERE a.id @@@ paradedb.boolean(should => ARRAY[{conj_clause}\
                              paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                              paradedb.match('texte', ${s_ph})])
                          AND a.status = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND t.jurisdiction = ANY(${codes_ph})
                        ORDER BY paradedb.score(a.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, -1 AS leg
                      FROM ctry"
                            ),
                        )
                    }
                    None => (String::new(), String::new(), String::new()),
                };
                // Jambe conteneurs optionnelle : `legal_text` SANS corps mais
                // à articles en vigueur (les codes — disjointe de la jambe
                // textes, bornée à `body IS NOT NULL`), titre en conjonctif
                // SEUL et sur la seule forme conjonctive de la REQUÊTE (pas
                // des expansions d'alias : « L442-1 du code de commerce »
                // étendu en « code de commerce » faisait voler le rang 1 par
                // le conteneur sur une requête d'article nommé), bornée aux natures
                // navigables comme un code (le filtre du catalogue `/codes`,
                // ADR 0133 — sinon tout décret/arrêté dont le titre matche
                // vole un rang : « 15 … loi du 6 juillet 1989 » matchait un
                // décret de 2025 par sa DATE) et au prior de juridiction quand
                // il est actif (les conteneurs étrangers ne sortent que pays
                // nommé). `leg = -1` : à rang RRF égal le conteneur prime
                // (requête navigationnelle).
                let mut cont_alts: Vec<String> = Vec::new();
                if container {
                    cont_alts.extend(lj_core::aliases::conj_title_query(query));
                    if container_alias {
                        cont_alts.extend(
                            lj_core::aliases::whole_query_expansions(query)
                                .iter()
                                .filter_map(|e| lj_core::aliases::conj_title_query(e)),
                        );
                    }
                    cont_alts.dedup();
                }
                let (cont_cte, cont_union) = match cont_alts.as_slice() {
                    [_, ..] => {
                        let clauses: Vec<String> = cont_alts
                            .iter()
                            .map(|alt| {
                                let ph = params.push(Box::new(alt.clone()));
                                format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                            })
                            .collect();
                        let conj = match clauses.as_slice() {
                            [single] => single.clone(),
                            many => {
                                format!("paradedb.boolean(should => ARRAY[{}])", many.join(", "))
                            }
                        };
                        let cont_juris = prim_ph
                            .map(|ph| format!("\n  AND t.jurisdiction = ANY(${ph})"))
                            .unwrap_or_default();
                        (
                            format!(
                                "
                    cont AS (
                      SELECT z.*, row_number() OVER (ORDER BY z.score DESC, z.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ {conj}
                          AND t.body IS NULL
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                          AND EXISTS (SELECT 1 FROM legal_article a
                                      WHERE a.text_uid = t.text_uid
                                        AND a.status = 'VIGUEUR'){cont_juris}
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) z
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, NULL AS texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, -2 AS leg
                      FROM cont"
                            ),
                        )
                    }
                    [] => (String::new(), String::new()),
                };
                // Chaque jambe est classée seule (le `row_number` re-trie le
                // top-`leg_limit`, l'ORDER BY interne reste sur le seul score
                // pour garder le Top-K scan ParadeDB), puis fusion par rang.
                // Tiebreak : jambe articles d'abord (recherche titre-primaire).
                format!(
                    "
                    WITH{cont_cte}{ctry_cte}{artf_cte}{usage_cte}
                    art AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        (SELECT t.slug, a.num, a.num_key, t.title, a.title_path, a.texte,
                               paradedb.score(a.id) AS score
                        FROM legal_article a
                        JOIN legal_text t ON t.text_uid = a.text_uid
                        WHERE {art_pred}
                          AND a.status = 'VIGUEUR'
                          AND t.slug IS NOT NULL{art_juris}{art_ctry_excl}
                        ORDER BY paradedb.score(a.id) DESC
                        LIMIT ${leg_ph}){art_merge_union}
                      ) x
                    ),
                    txt AS (
                      SELECT y.*, row_number() OVER (ORDER BY y.score DESC, y.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, t.body AS texte, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE {txt_pred}
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) y
                    )
                    SELECT u.slug, u.num, u.title, u.title_path, u.texte FROM (
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 0 AS leg
                      FROM art
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, texte,
                             ${w_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 1 AS leg
                      FROM txt{artf_union}{usage_union}{ctry_union}{cont_union}
                    ) u
                    {final_order}
                    LIMIT ${limit_ph}
                    "
                )
            }
        };

        let rows = self.conn.query(&sql, &params.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| match rrf {
                None => ArticleRankHit {
                    slug: r.get(0),
                    num: r.get(1),
                    code_title: r.get(3),
                    title_path: r.get(4),
                    texte: r.get(5),
                },
                Some(_) => ArticleRankHit {
                    slug: r.get(0),
                    num: r.get(1),
                    code_title: r.get(2),
                    title_path: r.get(3),
                    texte: r.get(4),
                },
            })
            .collect())
    }

    /// Catalogue des codes (ADR 0114, `/codes`) : tout `legal_text` à slug + son nombre
    /// d'articles en vigueur (`status = 'VIGUEUR'`, distincts par `num_key`). Trié par
    /// titre. Champs bruts (mapping `CodeCatalogueEntry` côté `lj-api`).
    /// Compteurs corpus normatif pour la home (`GET /api/corpus-stats`) : nombre
    /// total de textes (`legal_text`, toutes natures) et d'articles en vigueur
    /// (identités `(text_uid, num_key)` distinctes). Corpus entier, pas le seul
    /// catalogue navigable `/codes`. Un aller-retour, appelé 2×/jour derrière le
    /// cache TTL — les deux comptes exacts sortent d'une requête.
    #[tracing::instrument(name = "db.count_normative_corpus", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_normative_corpus(&self) -> Result<(i64, i64)> {
        let row = self
            .conn
            .query_one(
                "
                SELECT
                  (SELECT count(*) FROM legal_text) AS texts,
                  (SELECT count(DISTINCT (text_uid, num_key)) FROM legal_article
                   WHERE status = 'VIGUEUR') AS articles
                ",
                &[],
            )
            .await?;
        Ok((row.get(0), row.get(1)))
    }

    #[tracing::instrument(name = "db.list_legal_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn list_legal_texts(&self) -> Result<Vec<LegalTextCatalogRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT t.text_uid, t.slug, t.title, t.nature, t.jurisdiction,
                       (SELECT count(DISTINCT a.num_key)
                        FROM legal_article a
                        WHERE a.text_uid = t.text_uid AND a.status = 'VIGUEUR') AS article_count
                FROM legal_text t
                WHERE t.slug IS NOT NULL
                  -- Catalogue = textes *navigables comme un code* (ADR 0133) : codes
                  -- (`code*`/`CODE`), constitutions, lois/ordonnances. On écarte les
                  -- actes unitaires (arrêtés, décrets) et les conventions collectives
                  -- (`TI`/`IDCC`) : cités, mais pas parcourus comme un corpus.
                  AND (t.nature ILIKE 'code%'
                       OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI',
                           'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT',
                           'ETAT_CIVIL'))
                  -- … et seulement ceux qui ont AU MOINS un article en vigueur : on
                  -- écarte les versions abrogées/transférées (tous articles ABROGE/
                  -- MODIFIE/PÉRIMÉ → 0 en vigueur) et les coquilles sans articles. Elles
                  -- doublonnent le code vivant avec un « 0 article » non navigable.
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid AND a.status = 'VIGUEUR')
                ORDER BY t.title
                ",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| LegalTextCatalogRow {
                text_uid: r.get(0),
                slug: r.get(1),
                title: r.get(2),
                nature: r.get(3),
                jurisdiction: r.get(4),
                article_count: r.get(5),
            })
            .collect())
    }

    /// Table des matières d'un code (ADR 0114, sommaire) : la version en vigueur de
    /// chaque article (une ligne par `num_key`), triée par `position` (ordre de lecture
    /// réel, `NULLS LAST`) puis **tri naturel** du `num_key`
    /// ([`lj_core::article_order`]) — le tri lexical mettait « L. 10 » avant « L. 2 »
    /// et les décrets avant la partie législative. Léger (pas de corps : clic =
    /// navigation `/texte/{slug}/{num}`). Champs bruts (mapping `TocEntry` côté `lj-api`).
    #[tracing::instrument(name = "db.code_table_of_contents", skip(self), fields(db.system = "postgresql"))]
    pub async fn code_table_of_contents(&self, text_uid: &str) -> Result<Vec<TocArticleRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT num, num_key, title_path, status, position FROM (
                  -- VIGUEUR préférée, sinon dernière version (ADR 0162 §5).
                  SELECT DISTINCT ON (num_key) num, num_key, title_path, status,
                         position, date_debut
                  FROM legal_article
                  WHERE text_uid = $1
                  ORDER BY num_key, (status = 'VIGUEUR') DESC,
                           date_debut DESC NULLS LAST
                ) v
                ",
                &[&text_uid],
            )
            .await?;
        let mut rows: Vec<TocArticleRow> = rows
            .iter()
            .map(|r| TocArticleRow {
                num: r.get(0),
                num_key: r.get(1),
                title_path: r.get(2),
                status: r.get(3),
                position: r.get(4),
            })
            .collect();
        if rows.iter().all(|r| r.position.is_none()) {
            // Codes LEGI (aucune position) : ordre de lecture par divisions
            // médianes + tri naturel des num_key.
            lj_core::article_order::sort_reading_order(
                &mut rows,
                |r| &r.num_key,
                |r| r.title_path.as_deref(),
            );
        } else {
            rows.sort_by_cached_key(|r| {
                (
                    r.position.is_none(),
                    r.position,
                    num_key_sort_key(&r.num_key),
                )
            });
        }
        Ok(rows)
    }

    /// Vue-lecture à plat d'un texte sans structure ingérée (BOFiP,
    /// circulaires…) : la version en vigueur de chaque article avec son corps,
    /// dans le même ordre de lecture que [`Self::code_table_of_contents`].
    /// Champs au format [`TocReadingRow`] (que des articles, profondeur 1).
    #[tracing::instrument(name = "db.flat_text_reading", skip(self), fields(db.system = "postgresql"))]
    pub async fn flat_text_reading(&self, text_uid: &str) -> Result<Vec<TocReadingRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT num, num_key, title_path, status, position, texte, nota FROM (
                  -- VIGUEUR préférée, sinon dernière version (ADR 0162 §5).
                  SELECT DISTINCT ON (num_key) num, num_key, title_path, status,
                         position, texte, nota, date_debut
                  FROM legal_article
                  WHERE text_uid = $1
                  ORDER BY num_key, (status = 'VIGUEUR') DESC,
                           date_debut DESC NULLS LAST
                ) v
                ",
                &[&text_uid],
            )
            .await?;
        struct Row {
            num: String,
            num_key: String,
            title_path: Option<String>,
            status: String,
            position: Option<i32>,
            texte: Option<String>,
            nota: Option<String>,
        }
        let mut rows: Vec<Row> = rows
            .iter()
            .map(|r| Row {
                num: r.get(0),
                num_key: r.get(1),
                title_path: r.get(2),
                status: r.get(3),
                position: r.get(4),
                texte: r.get(5),
                nota: r.get(6),
            })
            .collect();
        if rows.iter().all(|r| r.position.is_none()) {
            lj_core::article_order::sort_reading_order(
                &mut rows,
                |r| &r.num_key,
                |r| r.title_path.as_deref(),
            );
        } else {
            rows.sort_by_cached_key(|r| {
                (
                    r.position.is_none(),
                    r.position,
                    num_key_sort_key(&r.num_key),
                )
            });
        }
        Ok(rows
            .into_iter()
            .map(|r| TocReadingRow {
                depth: 1,
                child_kind: "article".to_string(),
                child_cid: None,
                child_num_key: Some(r.num_key),
                label: r.num,
                etat: r.status,
                texte: r.texte,
                nota: r.nota,
            })
            .collect())
    }

    /// Articles voisins de `num_key` pour le contexte de lecture (ADR 0114), façon
    /// Légifrance et **adaptatif** : texte court ⇒ tous les articles ; sinon la
    /// division enclosante (même `title_path`) ; division trop grosse ⇒ fenêtre de
    /// `±CONTEXT_WINDOW` autour de la `position`. Léger (pas de corps : clic =
    /// navigation `/texte/{slug}/{num}`). Version servie = celle à `date` (ou
    /// `VIGUEUR`), dédoublonnée par `num_key`, triée par `position` puis tri
    /// naturel du `num_key` ([`lj_core::article_order`]) — même ordre que le
    /// sommaire, la sélection de fenêtre en dépend.
    #[tracing::instrument(name = "db.article_context", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_context(
        &self,
        text_uid: &str,
        num_key: &str,
        date: Option<NaiveDate>,
    ) -> Result<Vec<ArticleNeighborRow>> {
        let rows = match date {
            Some(d) => {
                self.conn
                    .query(
                        "
                        SELECT num, num_key, status, title_path, position FROM (
                          SELECT DISTINCT ON (num_key) num, num_key, status, title_path,
                                 position, date_debut
                          FROM legal_article
                          WHERE text_uid = $1 AND date_debut <= $2
                            AND (date_fin IS NULL OR date_fin >= $2)
                          ORDER BY num_key, date_debut DESC
                        ) v
                        ",
                        &[&text_uid, &d],
                    )
                    .await?
            }
            None => {
                self.conn
                    .query(
                        "
                        SELECT num, num_key, status, title_path, position FROM (
                          SELECT DISTINCT ON (num_key) num, num_key, status, title_path,
                                 position, date_debut
                          FROM legal_article
                          WHERE text_uid = $1 AND status = 'VIGUEUR'
                          ORDER BY num_key, date_debut DESC
                        ) v
                        ",
                        &[&text_uid],
                    )
                    .await?
            }
        };
        let mut lite: Vec<ContextLite> = rows
            .iter()
            .map(|r| ContextLite {
                num: r.get(0),
                num_key: r.get(1),
                status: r.get(2),
                title_path: r.get(3),
                position: r.get(4),
            })
            .collect();
        lite.sort_by_cached_key(|r| {
            (
                r.position.is_none(),
                r.position,
                num_key_sort_key(&r.num_key),
            )
        });
        Ok(select_article_context(lite, num_key))
    }
}

/// Ligne légère pour le calcul du contexte (triée avant sélection).
struct ContextLite {
    num: String,
    num_key: String,
    status: String,
    title_path: Option<String>,
    position: Option<i32>,
}

/// Constantes du ranking de la recherche d'articles (ADR 0232), calées par le
/// banc qrels `lj-bench article-rank-eval` (gt/articles/) : boost du titre
/// conjonctif, poids du filet titre OR (0,25 = max-min mesuré : au-delà le
/// filet ré-enterre les cibles descriptives), constante RRF et profondeur de
/// jambe avant fusion.
const ARTICLE_TITLE_CONJ_BOOST: &str = "4";
const ARTICLE_TITLE_OR_BOOST: &str = "0.25";
/// Boost du match **conjonctif** corps d'une expansion concept→corps (ADR 0241) :
/// la formule statutaire (« frais exposés non compris dans les dépens ») doit
/// matcher en entier pour remonter l'article gouvernant, à parité de poids avec
/// le titre conjonctif.
const ARTICLE_BODY_CONCEPT_BOOST: &str = "4";
/// Poids RRF de la jambe usage_terms (ADR 0248) : sacs de contextes de
/// citation, votes SOMMÉS avec les autres jambes (elle recouvre la jambe
/// articles — accumulation d'évidence). w=0,6 validé au banc (36 GT).
const ARTICLE_USAGE_WEIGHT: &str = "0.8";
const ARTICLE_RRF_K: &str = "60";
const ARTICLE_LEG_LIMIT: i64 = 200;
/// Poids RRF de la jambe articles étrangère (ADR 0234) : insensible entre
/// 0,1 et 0,5 au banc (l'étranger ne remonte dans le top qu'à poids ~1).
const ARTICLE_FOREIGN_WEIGHT: &str = "0.25";

/// Natures « navigables comme un code » — miroir du filtre du catalogue
/// `/codes` (ADR 0133), alias `t` requis. Borne la jambe conteneurs : sans
/// elle, tout décret/arrêté dont le titre matche la conjonction vole un rang.
const NAVIGABLE_TEXT_NATURES_SQL: &str = "(t.nature ILIKE 'code%' \
     OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI', \
         'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT', \
         'ETAT_CIVIL'))";

/// Visibilité par défaut (ADR 0246 §6), alias `t` requis : la recherche ne
/// sert pas les parutions sans objet consultable — actes individuels
/// (nominations…), véhicules de publication, lois d'habilitation. Elles
/// restent résolvables par uid.
const TEXT_ROLE_VISIBLE_SQL: &str = "t.role NOT IN ('individuel', 'vehicule', 'habilitation')";

/// Juridictions primaires d'une requête (ADR 0234) : le droit applicable en
/// France (FR/UE/INTL) + les pays que la requête nomme
/// ([`lj_core::jurisdictions::query_jurisdictions`]).
fn primary_jurisdictions(query: &str) -> Vec<String> {
    let mut primary: Vec<String> = ["FR", "UE", "INTL"].map(String::from).to_vec();
    primary.extend(
        lj_core::jurisdictions::query_jurisdictions(query)
            .into_iter()
            .map(String::from),
    );
    primary
}

/// Prédicats BM25 des deux jambes de la recherche d'articles et de ses
/// facettes (ADR 0114/0232). Par jambe (`legal_article_bm25` /
/// `legal_text_body_bm25`), `should` de trois clauses : titre **conjonctif
/// normalisé** boosté ([`lj_core::aliases::conj_title_query`] par alternative
/// requête/expansion — chaque alternative doit matcher le titre en entier),
/// filet titre OR à faible poids ($1, requête enrichie des expansions), corps
/// ($2, requête seule). Pousse les alternatives conjonctives dans `params`
/// ($3..) ; requête réduite à rien (« article ») → filet OR seul. Le
/// `t.body IS NOT NULL` borne la jambe textes aux familles à corps
/// (circulaires… — l'index couvre tous les titres).
fn article_search_predicates(
    params: &mut ArticleSearchParams,
    query: &str,
    expansions: &[String],
) -> (String, String) {
    let mut conj_alts: Vec<String> = std::iter::once(query.to_string())
        .chain(expansions.iter().cloned())
        .filter_map(|q| lj_core::aliases::conj_title_query(&q))
        .collect();
    conj_alts.dedup();
    let alt_phs: Vec<usize> = conj_alts
        .into_iter()
        .map(|a| params.push(Box::new(a)))
        .collect();
    let title_clauses = |field: &str| {
        let or = format!("paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('{field}', $1))");
        if alt_phs.is_empty() {
            return or;
        }
        let alts: Vec<String> = alt_phs
            .iter()
            .map(|ph| format!("paradedb.match('{field}', ${ph}, conjunction_mode => true)"))
            .collect();
        let conj = match alts.as_slice() {
            [single] => single.clone(),
            many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
        };
        format!("paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, {conj}), {or}")
    };
    let concept_phs = push_concept_expansions(params, query);
    // Jambe articles : prédicat 100 % indexé (`a.` seul) — le filtre de
    // visibilité `t.role` s'applique côté join `legal_text` chez les
    // consommateurs, pour que le scan ParadeDB reste sans heap filter.
    let art = format!(
        "a.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('texte', $2){}])",
        title_clauses("search_title"),
        concept_body_clause("texte", &concept_phs)
    );
    let txt = format!(
        "t.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('body', $2){}]) \
         AND t.body IS NOT NULL AND {TEXT_ROLE_VISIBLE_SQL}",
        title_clauses("title"),
        concept_body_clause("body", &concept_phs)
    );
    (art, txt)
}

/// Pousse les expansions concept→corps (ADR 0241) déclenchées par `query` et
/// renvoie leurs placeholders `$N`. « frais irrépétibles » → « frais exposés non
/// compris dans les dépens ». Vide si aucun synonyme déclenché.
fn push_concept_expansions(params: &mut ArticleSearchParams, query: &str) -> Vec<usize> {
    lj_core::aliases::concept_expansions(query)
        .into_iter()
        .map(|e| params.push(Box::new(e)))
        .collect()
}

/// Fragments SQL de clauses corps **conjonctives boostées** (ADR 0241) pour des
/// placeholders déjà poussés (`phs`) et un `field` (`texte`/`body`) : la formule
/// statutaire doit matcher en entier pour remonter l'article gouvernant, dont le
/// terme doctrinal est absent. Chaque fragment commence par `, ` pour s'insérer
/// dans un `ARRAY[…]` ; chaîne vide si `phs` vide.
fn concept_body_clause(field: &str, phs: &[usize]) -> String {
    phs.iter()
        .map(|ph| {
            format!(
                ", paradedb.boost({ARTICLE_BODY_CONCEPT_BOOST}, \
                 paradedb.match('{field}', ${ph}, conjunction_mode => true))"
            )
        })
        .collect()
}

/// Construit la jambe titre : requête enrichie des expansions d'alias (OR implicite
/// du `match`), substitut au sémantique.
fn article_title_query(query: &str, expansions: &[String]) -> String {
    if expansions.is_empty() {
        query.to_string()
    } else {
        format!("{query} {}", expansions.join(" "))
    }
}

/// Accumulateur de paramètres SQL pour la recherche d'articles et ses facettes.
/// `$1`/`$2` sont toujours la jambe titre et la jambe corps ; les filtres optionnels
/// (`text_uid`/`jurisdiction`/`nature`/`source`) puis les bornes de pagination sont
/// poussés à la suite, l'indexation des `$N` restant correcte quels que soient les
/// filtres présents. Valeurs `Box`ées (idiom dynamique, cf. [`as_param_refs`]).
struct ArticleSearchParams {
    params: Vec<Box<dyn ToSql + Sync + Send>>,
}

impl ArticleSearchParams {
    fn new(title_query: String, body_query: &str) -> Self {
        Self {
            params: vec![Box::new(title_query), Box::new(body_query.to_string())],
        }
    }

    /// Pousse une valeur et renvoie son numéro de placeholder `$N` (1-indexé).
    fn push(&mut self, value: Box<dyn ToSql + Sync + Send>) -> usize {
        self.params.push(value);
        self.params.len()
    }

    /// Pousse les filtres présents et renvoie les DEUX clauses SQL `AND …`
    /// (chaînes vides si aucun filtre) : une pour la jambe articles, une pour la
    /// jambe textes à corps (ADR 0196) — mêmes placeholders, colonnes propres à
    /// chaque jambe. Côté articles, toutes les colonnes vivent sur
    /// `legal_article` (dénormalisées, ADR 0254) et sont des champs de l'index
    /// BM25 : les filtres s'appliquent dans le scan, le prédicat reste 100 %
    /// indexé. `a.nature` est stockée en upper (le corpus curé mélange les
    /// casses) ; `source` vaut `a.source` côté articles et `lower(t.nature)`
    /// côté textes à corps (pas de diffuseur par ligne sur `legal_text` — la
    /// famille tient lieu de jeton). `nature_set` (sur-facette portée, valeurs
    /// upper) filtre les deux jambes : `(liste, true)` = nature DANS la liste,
    /// `(liste, false)` = nature HORS liste (le complément « norme » est un
    /// ensemble ouvert).
    fn push_filters(
        &mut self,
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
        nature_set: Option<(&[String], bool)>,
    ) -> (String, String) {
        let mut art = String::new();
        let mut txt = String::new();
        for (art_col, txt_col, value) in [
            ("a.text_uid", "t.text_uid", text_uid),
            ("a.jurisdiction", "t.jurisdiction", jurisdiction),
            (
                "a.nature",
                "upper(t.nature)",
                nature.map(str::to_uppercase).as_deref(),
            ),
            ("a.source", "lower(t.nature)", source),
        ] {
            if let Some(v) = value {
                let ph = self.push(Box::new(v.to_string()));
                art.push_str(&format!("\n  AND {art_col} = ${ph}"));
                txt.push_str(&format!("\n  AND {txt_col} = ${ph}"));
            }
        }
        if let Some((natures, include)) = nature_set.filter(|(n, _)| !n.is_empty()) {
            let ph = self.push(Box::new(natures.to_vec()));
            let op = if include { "= ANY" } else { "<> ALL" };
            art.push_str(&format!("\n  AND a.nature {op}(${ph})"));
            txt.push_str(&format!("\n  AND upper(t.nature) {op}(${ph})"));
        }
        (art, txt)
    }

    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        // `Send` sur les boxes garde le future de la méthode `Send` (frontière
        // axum Handler) ; on retombe ici sur `&(dyn ToSql + Sync)` attendu par
        // tokio-postgres (l'auto-trait `Send` est largué par coercion).
        self.params
            .iter()
            .map(|p| &**p as &(dyn ToSql + Sync))
            .collect()
    }
}

/// Texte court : on affiche tous les articles (DDHC, courtes lois/traités).
const CONTEXT_FULL_MAX: usize = 40;
/// Division enclosante affichée entière tant qu'elle ne dépasse pas ce seuil.
const CONTEXT_DIVISION_MAX: usize = 80;
/// Demi-fenêtre de repli (par position) quand la division est trop grosse.
const CONTEXT_WINDOW: usize = 6;

/// Sélection adaptative du contexte (ADR 0114), pure et testable. `rows` est déjà
/// trié par `position`. Renvoie le sous-ensemble à afficher, l'article courant
/// marqué `current`. Hiérarchie : tout (texte court) → division (même
/// `title_path`) → fenêtre `±CONTEXT_WINDOW`. `current_num_key` absent ⇒ vide.
fn select_article_context(
    rows: Vec<ContextLite>,
    current_num_key: &str,
) -> Vec<ArticleNeighborRow> {
    let n = rows.len();
    let Some(idx) = rows.iter().position(|r| r.num_key == current_num_key) else {
        return Vec::new();
    };
    let pick = |i: usize, rows: &[ContextLite]| ArticleNeighborRow {
        num: rows[i].num.clone(),
        num_key: rows[i].num_key.clone(),
        status: rows[i].status.clone(),
        current: i == idx,
    };

    if n <= CONTEXT_FULL_MAX {
        return (0..n).map(|i| pick(i, &rows)).collect();
    }
    let cur_tp = &rows[idx].title_path;
    if cur_tp.is_some() {
        let division: Vec<usize> = (0..n).filter(|&i| rows[i].title_path == *cur_tp).collect();
        if division.len() <= CONTEXT_DIVISION_MAX {
            return division.into_iter().map(|i| pick(i, &rows)).collect();
        }
    }
    let lo = idx.saturating_sub(CONTEXT_WINDOW);
    let hi = (idx + CONTEXT_WINDOW + 1).min(n);
    (lo..hi).map(|i| pick(i, &rows)).collect()
}

#[cfg(test)]
mod tests {
    use super::{select_article_context, ContextLite};

    fn lite(num: &str, tp: Option<&str>) -> ContextLite {
        ContextLite {
            num: num.to_string(),
            num_key: num.to_string(),
            status: "VIGUEUR".to_string(),
            title_path: tp.map(str::to_string),
            position: None,
        }
    }

    #[test]
    fn context_short_text_returns_all_articles() {
        // DDHC-like : 17 articles, sous le seuil FULL → tout, courant marqué.
        let rows: Vec<ContextLite> = (1..=17).map(|i| lite(&i.to_string(), None)).collect();
        let out = select_article_context(rows, "9");
        assert_eq!(out.len(), 17);
        assert_eq!(out.iter().filter(|r| r.current).count(), 1);
        assert!(out.iter().find(|r| r.num == "9").unwrap().current);
    }

    #[test]
    fn context_large_text_returns_enclosing_division() {
        // 200 articles, FULL dépassé ; la division de l'article courant compte 5
        // membres (≤ DIVISION_MAX) → on rend la division entière, pas la fenêtre.
        let mut rows: Vec<ContextLite> =
            (1..=100).map(|i| lite(&i.to_string(), Some("A"))).collect();
        rows.extend((101..=105).map(|i| lite(&i.to_string(), Some("B"))));
        rows.extend((106..=200).map(|i| lite(&i.to_string(), Some("C"))));
        let out = select_article_context(rows, "103");
        assert_eq!(out.len(), 5, "toute la division B");
        assert!(out
            .iter()
            .all(|r| ("101"..="105").contains(&r.num.as_str())));
        assert!(out.iter().find(|r| r.num == "103").unwrap().current);
    }

    #[test]
    fn context_huge_division_falls_back_to_window() {
        // 200 articles tous dans la même division (> DIVISION_MAX) → fenêtre ±6
        // autour de la position (13 articles), courant centré.
        let rows: Vec<ContextLite> = (1..=200).map(|i| lite(&i.to_string(), Some("A"))).collect();
        let out = select_article_context(rows, "100");
        assert_eq!(out.len(), 13);
        assert_eq!(out.first().unwrap().num, "94");
        assert_eq!(out.last().unwrap().num, "106");
        assert!(out.iter().find(|r| r.num == "100").unwrap().current);
    }

    #[test]
    fn context_unknown_article_is_empty() {
        let rows: Vec<ContextLite> = (1..=200).map(|i| lite(&i.to_string(), Some("A"))).collect();
        assert!(select_article_context(rows, "999").is_empty());
    }
}
