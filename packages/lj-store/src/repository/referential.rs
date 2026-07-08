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
    ArticleNeighborRow, ArticleSearchRow, ArticleSearchStats, CitingDecisionRow, FacetCount,
    FacetValueRow, JurisdictionRow, LawCodeSummaryRow, LawVersionRow, LegalArticleRow,
    LegalTextCatalogRow, LegalTextRow, TocArticleRow,
};
use super::DecisionRepository;
use crate::error::Result;
use chrono::NaiveDate;
use tokio_postgres::types::ToSql;

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
    /// unité juridictionnelle (`tj76351`, `ca_paris`, `cass_soc`…) avec type,
    /// ville et label FR. Alimente le cache référentiel in-process de `lj-api`.
    #[tracing::instrument(name = "db.load_jurisdictions", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_jurisdictions(&self) -> Result<Vec<JurisdictionRow>> {
        let rows = self
            .conn
            .query(
                "SELECT code, juridiction_type, city, label FROM jurisdiction",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| JurisdictionRow {
                code: r.get(0),
                juridiction_type: r.get(1),
                city: r.get(2),
                label: r.get(3),
            })
            .collect())
    }

    /// Upsert idempotent (#7) d'un texte de loi sur son identité `text_uid`. Réécrit
    /// le catalogue à chaque incrément ; rejoue sans dupliquer. `title_key` (=
    /// `normalize_instrument(title)`) est calculé par l'appelant et stocké tel quel.
    ///
    /// Ne touche jamais `slug` : un slug est immuable une fois posé (ADR 0162) et
    /// son unique écrivain est la passe [`Self::set_text_slugs`].
    #[tracing::instrument(name = "db.upsert_legal_text", skip(self, text), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_text(&self, text: &LegalTextRow) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (text_uid) DO UPDATE SET
                  jurisdiction = EXCLUDED.jurisdiction,
                  title = EXCLUDED.title,
                  title_key = EXCLUDED.title_key,
                  nature = EXCLUDED.nature,
                  last_modified = EXCLUDED.last_modified,
                  date_texte = EXCLUDED.date_texte,
                  date_publi = EXCLUDED.date_publi,
                  eli = EXCLUDED.eli,
                  nor = EXCLUDED.nor,
                  instrument_key = EXCLUDED.instrument_key
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
                ],
            )
            .await?;
        Ok(())
    }

    /// Textes sans slug, `(text_uid, title)` triés par `text_uid` — l'ordre
    /// déterministe de la passe d'assignation (ADR 0162).
    pub async fn texts_without_slug(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title FROM legal_text WHERE slug IS NULL ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
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
    /// Renvoie `true` si la ligne a été insérée ou modifiée, `false` si skip.
    #[tracing::instrument(name = "db.upsert_legal_article", skip(self, art), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_article(&self, art: &LegalArticleRow) -> Result<bool> {
        let checksum = i64::from_ne_bytes(art.content_checksum.to_ne_bytes());
        let row = self
            .conn
            .query_opt(
                "
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
                RETURNING 1
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
        Ok(row.is_some())
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
                        SELECT 1 FROM legal_citation c WHERE c.ref_text_uid = t.text_uid)
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
                        WHERE a.text_uid = t.text_uid) AS article_count
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
        }))
    }

    /// Résout un slug de code en `text_uid` par lookup **exact** (ADR 0112 §6 /
    /// ADR 0123 §2).
    ///
    /// `slug` = la chaîne d'URL `/loi/{slug}` — un slug canonique que **nos liens
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
    /// `legal_citation` (index `idx_lc_ref`). Triée par `date_lecture`
    /// décroissante, paginée. Champs bruts (mapping DTO côté `lj-api`).
    #[tracing::instrument(name = "db.law_decisions_citing", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_decisions_citing(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CitingDecisionRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT DISTINCT d.id, d.public_id, d.juridiction_type,
                       d.jurisdiction_name, d.date_lecture::text, d.docket_numbers
                FROM legal_citation lc
                JOIN decisions d ON d.id = lc.decision_id
                WHERE lc.ref_text_uid = $1 AND lc.ref_num_key = $2
                -- ORDER BY sur l'expression *castée* (= celle de la liste SELECT
                -- DISTINCT) : Postgres exige que les expr d'ORDER BY figurent dans la
                -- liste de sélection. Texte ISO 'YYYY-MM-DD' → tri == chronologique.
                ORDER BY d.date_lecture::text DESC NULLS LAST, d.id
                LIMIT $3 OFFSET $4
                ",
                &[&ref_text_uid, &num_key, &limit, &offset],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CitingDecisionRow {
                id: r.get(0),
                public_id: r.get(1),
                juridiction_type: r.get(2),
                jurisdiction_name: r.get(3),
                date_lecture: r.get(4),
                docket_numbers: r.get(5),
            })
            .collect())
    }

    /// Itère `(slug, num, lastmod)` pour les articles en vigueur (sitemaps
    /// `/loi/{slug}/{num}`, ADR 0112). `lastmod` = `COALESCE(t.last_modified,
    /// a.date_debut, '1970-01-01')`. Ordre déterministe `(slug, num)` ; pas de
    /// pagination SQL — `build_sitemaps` pagine en mémoire.
    #[tracing::instrument(name = "db.iter_referential_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_referential_for_sitemap(&self) -> Result<Vec<(String, String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "
                SELECT t.slug, a.num_key,
                       COALESCE(t.last_modified, a.date_debut, DATE '1970-01-01')::date AS lastmod
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

    /// Rafraîchit `code_title` (titre du code parent dénormalisé) depuis
    /// `legal_text` pour les articles dont il diffère (ADR 0114). Appelé en fin
    /// d'ingest référentiel : LEGI streame articles et codes séparément, l'article
    /// n'a pas le titre du code au parse → on le pose ici. La colonne générée
    /// `search_title` (titre formé indexé) est recalculée par Postgres sur les
    /// lignes touchées. Renvoie le nombre de lignes mises à jour. Idempotent (#7).
    #[tracing::instrument(name = "db.refresh_article_code_titles", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_article_code_titles(&self) -> Result<u64> {
        // `UPDATE` global sur tout `legal_article` (~M lignes) : le scan du join
        // dépasse le `statement_timeout` du pool (30 s) dès que le corpus grossit
        // (observé sur load-legal-corpus). On le lève **localement**, dans une
        // transaction dédiée — l'UPDATE n'écrit que les lignes dont le titre a
        // dérivé, reste idempotent (#7).
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let n = self
                .conn
                .execute(
                    "UPDATE legal_article a SET code_title = t.title \
                     FROM legal_text t \
                     WHERE a.text_uid = t.text_uid \
                       AND a.code_title IS DISTINCT FROM t.title",
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

    /// Recherche plein-texte d'articles (ADR 0114, `/recherche-textes`), **titre-
    /// primaire**. Jambe titre = `search_title` (titre formé : code + n° + division)
    /// boostée, requête enrichie des expansions d'alias OR-ées (acronymes/noms
    /// usuels, substitut au sémantique) ; jambe corps = `texte` (requête seule,
    /// secondaire). Fusion par `paradedb.boolean(should)` — pas de RRF (BM25 unique,
    /// score comparable). Articles `VIGUEUR`, optionnellement bornés à un `text_uid`.
    /// `slug`/`code_title` joints pour le lien `/loi/{slug}/{num}`.
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
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ArticleSearchRow>> {
        // `$1` titre (boosté), `$2` corps, puis les filtres optionnels en `$3..`.
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let filters = params.push_filters(text_uid, jurisdiction, nature, source);
        let limit_ph = params.push(Box::new(limit));
        let offset_ph = params.push(Box::new(offset));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT a.text_uid, t.slug, t.title, a.num, a.num_key,
                           a.title_path, a.status, a.source, a.texte,
                           paradedb.score(a.id) AS score
                    FROM legal_article a
                    JOIN legal_text t ON t.text_uid = a.text_uid
                    WHERE {ARTICLE_SEARCH_PREDICATE}
                      AND a.status = 'VIGUEUR'
                      AND t.slug IS NOT NULL{filters}
                    ORDER BY paradedb.score(a.id) DESC, a.num_key
                    LIMIT ${limit_ph} OFFSET ${offset_ph}
                    "
                ),
                &params.refs(),
            )
            .await?;
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

    /// Total exact + les trois facettes de la recherche d'articles (ADR 0114) en
    /// **une** requête `GROUPING SETS`, sous le même prédicat BM25 + filtres que
    /// [`Self::search_articles`] : le prédicat (le poste dominant, ~600 ms sur le
    /// corpus complet) ne s'exécute qu'une fois au lieu de quatre. Les comptes
    /// restent ceux du prédicat complet (filtres inclus) : l'UI montre la
    /// composition du résultat courant, pas un univers non filtré. Chaque axe est
    /// trié count décroissant puis valeur ascendante ; les valeurs vides sont
    /// écartées. `nature` est normalisée `upper()` (le corpus curé mélange
    /// `LOI`/`loi`, `CONSTITUTION`/`constitution`).
    #[tracing::instrument(name = "db.article_search_stats", skip(self, expansions), fields(db.system = "postgresql"))]
    pub async fn article_search_stats(
        &self,
        query: &str,
        expansions: &[String],
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
    ) -> Result<ArticleSearchStats> {
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let filters = params.push_filters(text_uid, jurisdiction, nature, source);
        // `GROUPING(...)` encode le set actif en bitmask (bit levé = colonne NON
        // groupée) : jurisdiction = 0b011, nature = 0b101, source = 0b110, total
        // (grand agrégat) = 0b111.
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT GROUPING(t.jurisdiction, upper(t.nature), a.source) AS gset,
                           t.jurisdiction, upper(t.nature) AS nature, a.source,
                           count(*) AS n
                    FROM legal_article a
                    JOIN legal_text t ON t.text_uid = a.text_uid
                    WHERE {ARTICLE_SEARCH_PREDICATE}
                      AND a.status = 'VIGUEUR'
                      AND t.slug IS NOT NULL{filters}
                    GROUP BY GROUPING SETS ((t.jurisdiction), (upper(t.nature)), (a.source), ())
                    "
                ),
                &params.refs(),
            )
            .await?;
        let mut stats = ArticleSearchStats {
            total: 0,
            jurisdiction: Vec::new(),
            nature: Vec::new(),
            source: Vec::new(),
        };
        for r in &rows {
            let gset: i32 = r.get(0);
            let count: i64 = r.get(4);
            let (axis, value): (&mut Vec<FacetCount>, Option<String>) = match gset {
                0b011 => (&mut stats.jurisdiction, r.get(1)),
                0b101 => (&mut stats.nature, r.get(2)),
                0b110 => (&mut stats.source, r.get(3)),
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
            &mut stats.jurisdiction,
            &mut stats.nature,
            &mut stats.source,
        ] {
            axis.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        }
        Ok(stats)
    }

    /// Catalogue des codes (ADR 0114, `/codes`) : tout `legal_text` à slug + son nombre
    /// d'articles en vigueur (`status = 'VIGUEUR'`, distincts par `num_key`). Trié par
    /// titre. Champs bruts (mapping `CodeCatalogueEntry` côté `lj-api`).
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
    /// réel, `NULLS LAST`) puis `num_key`. Léger (pas de corps : clic = navigation
    /// `/loi/{slug}/{num}`). Champs bruts (mapping `TocEntry` côté `lj-api`).
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
                ORDER BY position NULLS LAST, num_key
                ",
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TocArticleRow {
                num: r.get(0),
                num_key: r.get(1),
                title_path: r.get(2),
                status: r.get(3),
                position: r.get(4),
            })
            .collect())
    }

    /// Articles voisins de `num_key` pour le contexte de lecture (ADR 0114), façon
    /// Légifrance et **adaptatif** : texte court ⇒ tous les articles ; sinon la
    /// division enclosante (même `title_path`) ; division trop grosse ⇒ fenêtre de
    /// `±CONTEXT_WINDOW` autour de la `position`. Léger (pas de corps : clic =
    /// navigation `/loi/{slug}/{num}`). Version servie = celle à `date` (ou
    /// `VIGUEUR`), dédoublonnée par `num_key`, triée par `position`.
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
                        SELECT num, num_key, status, title_path FROM (
                          SELECT DISTINCT ON (num_key) num, num_key, status, title_path,
                                 position, date_debut
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
                        SELECT num, num_key, status, title_path FROM (
                          SELECT DISTINCT ON (num_key) num, num_key, status, title_path,
                                 position, date_debut
                          FROM legal_article
                          WHERE text_uid = $1 AND status = 'VIGUEUR'
                          ORDER BY num_key, date_debut DESC
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        ",
                        &[&text_uid],
                    )
                    .await?
            }
        };
        let lite: Vec<ContextLite> = rows
            .iter()
            .map(|r| ContextLite {
                num: r.get(0),
                num_key: r.get(1),
                status: r.get(2),
                title_path: r.get(3),
            })
            .collect();
        Ok(select_article_context(lite, num_key))
    }
}

/// Ligne légère pour le calcul du contexte (déjà triée par `position`).
struct ContextLite {
    num: String,
    num_key: String,
    status: String,
    title_path: Option<String>,
}

/// Prédicat BM25 commun à la recherche d'articles et à ses facettes (ADR 0114,
/// recherche titre-primaire). `$1` = jambe titre (`search_title` boostée, requête +
/// expansions d'alias), `$2` = jambe corps (`texte`, requête seule). Fusion par
/// `paradedb.boolean(should)` — pas de RRF (BM25 unique, score comparable). Le boost
/// `4` du titre est au jugé : le titre formé (code + n° + division) prime nettement.
const ARTICLE_SEARCH_PREDICATE: &str = "a.id @@@ paradedb.boolean(should => ARRAY[\
     paradedb.boost(4, paradedb.match('search_title', $1)),\
     paradedb.match('texte', $2)\
   ])";

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

    /// Pousse les filtres présents et renvoie la clause SQL `AND …` correspondante
    /// (chaîne vide si aucun filtre). `jurisdiction`/`nature` portés par `legal_text t`,
    /// `source` par `legal_article a`. `nature` se compare en `upper()` des deux côtés
    /// — la facette expose la valeur normalisée majuscule (cf.
    /// [`DecisionRepository::article_search_stats`]).
    fn push_filters(
        &mut self,
        text_uid: Option<&str>,
        jurisdiction: Option<&str>,
        nature: Option<&str>,
        source: Option<&str>,
    ) -> String {
        let mut clause = String::new();
        for (column, value) in [
            ("a.text_uid", text_uid),
            ("t.jurisdiction", jurisdiction),
            ("upper(t.nature)", nature.map(str::to_uppercase).as_deref()),
            ("a.source", source),
        ] {
            if let Some(v) = value {
                let ph = self.push(Box::new(v.to_string()));
                clause.push_str(&format!("\n  AND {column} = ${ph}"));
            }
        }
        clause
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
