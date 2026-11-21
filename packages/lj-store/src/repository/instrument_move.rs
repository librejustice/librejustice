//! Identité et contenu d'instrument : routes `publie` lues par la passe
//! articles JORF, déménagement de chronique LEGI, re-frappe et fusion d'uids
//! par namespace d'ordre juridique. Tout se fait par uid **explicite** — jamais
//! par résolution de clé : les `title_key` collisionnent.

use chrono::NaiveDate;
use lj_norm::text_aliases::TextAlias;

use super::DecisionRepository;
use crate::error::{Result, StoreError};

/// Lignes déplacées par un renommage d'uid de fiche : articles, arêtes TOC,
/// liens (owner + cibles au grain texte et au grain libre), citations
/// texte→texte (owner + ref), citations texte→décision, usages d'articles.
#[derive(Debug, Default)]
pub struct TextRenameCounts {
    pub articles: u64,
    pub toc: u64,
    pub links_owner: u64,
    pub links_target: u64,
    pub links_target_uid: u64,
    pub text_cit_owner: u64,
    pub text_cit_ref: u64,
    pub case_cit: u64,
    pub usage: u64,
}

/// What merging a text into another as one of its versions would write, read before
/// anything is. `close` is the first date the target writes; a row whose window would
/// close on or before it opens counts in `empty_windows`, one the target already holds
/// under the same key in `collisions`.
#[derive(Debug)]
pub struct TextVersionMerge {
    pub from_title: String,
    pub from_slug: Option<String>,
    pub into_slug: Option<String>,
    pub from_articles: i64,
    pub into_articles: i64,
    pub shared_keys: i64,
    pub from_only_keys: i64,
    pub close: Option<NaiveDate>,
    pub empty_windows: i64,
    pub collisions: i64,
    pub owned_links: i64,
}

impl DecisionRepository<'_> {
    /// Couples `(véhicule JORFTEXT, fiche instrument)` des arêtes `publie`
    /// possédées par une fiche instrument hors namespace JORF (`INTL/…`, `FR/…`) —
    /// les arêtes `publie` du graphe DILA, possédées par un `JORFTEXT`, n'en sont
    /// pas. Lu entre les deux passes de l'ingest JORF : la passe articles route les
    /// `JORFARTI` d'un véhicule vers sa fiche instrument en suivant l'arête, sans
    /// lire la nature en base. Lecture seule, idempotent.
    #[tracing::instrument(name = "db.publie_vehicle_instrument_pairs", skip(self), fields(db.system = "postgresql"))]
    pub async fn publie_vehicle_instrument_pairs(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT target_text_uid, owner_text_uid FROM legal_link
                 WHERE verb = 'publie' AND direction = 'incoming'
                   AND target_text_uid LIKE 'JORFTEXT%'
                   AND owner_text_uid NOT LIKE 'JORFTEXT%'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Déménage la chronique LEGI d'un instrument que DILA consolide sous le cid du
    /// véhicule : lignes `LEGIARTI` + arêtes TOC. Réservé à une liste fermée de
    /// dossiers vérifiés à la main — aucun marqueur structurel ne distingue un
    /// dossier LEGI « consolidation du décret » (dispositif) d'une « consolidation
    /// de l'instrument » (chronique). `exclusions` : couples `(num_key,
    /// date_debut)` qui restent au véhicule, les jumeaux LEGI du dispositif quand
    /// le dossier mélange les deux couches.
    /// Re-runnable : la passe LEGI n'est pas routée, une retouche DILA du dossier
    /// redépose sur le véhicule — relancer re-migre les versions neuves et purge
    /// les doublons redéposés (même `(num_key, date_debut)` déjà sur l'instrument).
    /// Renvoie (déplacées, doublons purgés, TOC).
    #[tracing::instrument(name = "db.move_chronicle_articles", skip(self, exclusions), fields(db.system = "postgresql"))]
    pub async fn move_chronicle_articles(
        &self,
        vehicle_uid: &str,
        instrument_uid: &str,
        exclusions: &[(String, NaiveDate)],
    ) -> Result<(u64, u64, u64)> {
        let (excl_nums, excl_dates): (Vec<&str>, Vec<NaiveDate>) =
            exclusions.iter().map(|(n, d)| (n.as_str(), *d)).unzip();
        self.conn.batch_execute("BEGIN").await?;
        let moved: Result<(u64, u64, u64)> = async {
            let articles = self
                .conn
                .execute(
                    "UPDATE legal_article a SET text_uid = $2
                     WHERE a.text_uid = $1 AND a.source_uid LIKE 'LEGIARTI%'
                       AND NOT EXISTS (
                         SELECT 1 FROM unnest($3::text[], $4::date[]) AS e(k, d)
                         WHERE e.k = a.num_key AND e.d = a.date_debut
                       )
                       AND NOT EXISTS (
                         SELECT 1 FROM legal_article i
                         WHERE i.text_uid = $2 AND i.num_key = a.num_key
                           AND i.date_debut = a.date_debut
                       )",
                    &[&vehicle_uid, &instrument_uid, &excl_nums, &excl_dates],
                )
                .await?;
            let dupes = self
                .conn
                .execute(
                    "DELETE FROM legal_article a
                     WHERE a.text_uid = $1 AND a.source_uid LIKE 'LEGIARTI%'
                       AND NOT EXISTS (
                         SELECT 1 FROM unnest($2::text[], $3::date[]) AS e(k, d)
                         WHERE e.k = a.num_key AND e.d = a.date_debut
                       )",
                    &[&vehicle_uid, &excl_nums, &excl_dates],
                )
                .await?;
            let toc = self
                .conn
                .execute(
                    "UPDATE legal_toc_edge SET text_uid = $2 WHERE text_uid = $1",
                    &[&vehicle_uid, &instrument_uid],
                )
                .await?;
            Ok((articles, dupes, toc))
        }
        .await;
        match moved {
            Ok(counts) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Renomme l'uid d'une fiche et cascade sur toutes les colonnes porteuses. La
    /// ligne `legal_text` est copiée sous le nouvel uid (slug et identité
    /// inchangés — les URLs publiques restent stables), les tables filles
    /// basculées, l'ancienne ligne supprimée — une transaction par fiche.
    /// `jurisdiction` remplace la colonne si fournie (cas du re-routage `INTL`).
    /// Les citations des décisions (`legal_citation.spans`) ne sont pas cascadées :
    /// le drain `reextract-fields --full --field legal_references` les ré-émet vers
    /// le nouvel uid via le linker. Idempotent : source absente + cible en place →
    /// `None`.
    #[tracing::instrument(name = "db.rename_legal_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn rename_legal_text(
        &self,
        from_uid: &str,
        to_uid: &str,
        jurisdiction: Option<&str>,
    ) -> Result<Option<TextRenameCounts>> {
        self.conn.batch_execute("BEGIN").await?;
        let renamed: Result<Option<TextRenameCounts>> = async {
            let source = self
                .conn
                .query_opt(
                    "SELECT slug FROM legal_text WHERE text_uid = $1 FOR UPDATE",
                    &[&from_uid],
                )
                .await?;
            let target = self
                .conn
                .query_opt("SELECT 1 FROM legal_text WHERE text_uid = $1", &[&to_uid])
                .await?;
            let slug: Option<String> = match (&source, target.is_some()) {
                (Some(row), _) => row.get("slug"),
                _ => None,
            };
            match (source.is_some(), target.is_some()) {
                (true, false) => {}
                (false, true) => return Ok(None),
                (true, true) => {
                    return Err(StoreError::Invalid(format!(
                        "cible déjà au catalogue : {to_uid} (source {from_uid} encore présente)"
                    )))
                }
                (false, false) => {
                    return Err(StoreError::Invalid(format!(
                        "fiche source absente: {from_uid}"
                    )))
                }
            }
            self.conn
                .execute(
                    // `id` omis : la séquence en frappe un neuf (la fiche
                    // source vit encore, son id est pris). `slug` posé après
                    // la suppression de la source — l'index est unique.
                    "INSERT INTO legal_text (
                         text_uid, jurisdiction, slug, title, title_key, nature,
                         last_modified, date_texte, date_publi, eli, nor,
                         instrument_key, num_prefix_agnostic, upcoming_versions,
                         status, role
                     )
                     SELECT $2, COALESCE($3, t.jurisdiction), NULL, t.title,
                            t.title_key, t.nature, t.last_modified, t.date_texte,
                            t.date_publi, t.eli, t.nor, t.instrument_key,
                            t.num_prefix_agnostic, t.upcoming_versions,
                            t.status, t.role
                     FROM legal_text t WHERE t.text_uid = $1",
                    &[&from_uid, &to_uid, &jurisdiction],
                )
                .await?;
            self.stamp_legal_texts(&[to_uid.to_string()]).await?;
            let mut counts = TextRenameCounts::default();
            for (slot, sql) in [
                (
                    &mut counts.articles as &mut u64,
                    "UPDATE legal_article SET text_uid = $2 WHERE text_uid = $1",
                ),
                (
                    &mut counts.toc,
                    "UPDATE legal_toc_edge SET text_uid = $2 WHERE text_uid = $1",
                ),
                (
                    &mut counts.links_owner,
                    "UPDATE legal_link SET owner_text_uid = $2 WHERE owner_text_uid = $1",
                ),
                (
                    &mut counts.links_target,
                    "UPDATE legal_link SET target_text_uid = $2 WHERE target_text_uid = $1",
                ),
                (
                    &mut counts.text_cit_owner,
                    "UPDATE text_legal_citation SET owner_text_uid = $2 WHERE owner_text_uid = $1",
                ),
                (
                    &mut counts.text_cit_ref,
                    "UPDATE text_legal_citation SET ref_text_uid = $2 WHERE ref_text_uid = $1",
                ),
                (
                    &mut counts.case_cit,
                    "UPDATE text_case_citation SET owner_text_uid = $2 WHERE owner_text_uid = $1",
                ),
                (
                    &mut counts.usage,
                    "UPDATE legal_article_usage SET text_uid = $2 WHERE text_uid = $1",
                ),
                (
                    &mut counts.links_target_uid,
                    "UPDATE legal_link SET target_uid = $2 WHERE target_uid = $1",
                ),
            ] {
                *slot = self.conn.execute(sql, &[&from_uid, &to_uid]).await?;
            }
            // Before the delete: the redirect FK is `ON DELETE CASCADE`, so an
            // already published URL would vanish with the uid it points at.
            self.conn
                .execute(
                    "UPDATE legal_text_slug_redirect SET text_uid = $2 WHERE text_uid = $1",
                    &[&from_uid, &to_uid],
                )
                .await?;
            self.conn
                .execute("DELETE FROM legal_text WHERE text_uid = $1", &[&from_uid])
                .await?;
            if slug.is_some() {
                self.conn
                    .execute(
                        "UPDATE legal_text SET slug = $2 WHERE text_uid = $1",
                        &[&to_uid, &slug],
                    )
                    .await?;
            }
            Ok(Some(counts))
        }
        .await;
        match renamed {
            Ok(counts) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Fusionne une fiche jumelle dans la fiche cible qui porte déjà le même
    /// contenu (doublon `EU/` ↔ uid DILA d'un même acte). Le **contenu** du doublon
    /// est jeté — articles, arêtes TOC, citations sortantes, usages : la cible en
    /// porte les jumeaux. Seules les **entrantes** déménagent : liens dont il est
    /// la cible, citations texte→texte qui le référencent. Les citations des
    /// décisions suivent au drain de relink. Idempotent : source absente → `None`.
    /// Two things survive the fiche that dies: its slug, kept as a redirect, and
    /// the identity columns the target has none of, a DILA fiche carrying the
    /// date and the NOR that EUR-Lex does not publish.
    #[tracing::instrument(name = "db.collapse_legal_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn collapse_legal_text(
        &self,
        from_uid: &str,
        into_uid: &str,
    ) -> Result<Option<TextRenameCounts>> {
        self.conn.batch_execute("BEGIN").await?;
        let collapsed: Result<Option<TextRenameCounts>> = async {
            if self
                .conn
                .query_opt(
                    "SELECT 1 FROM legal_text WHERE text_uid = $1 FOR UPDATE",
                    &[&from_uid],
                )
                .await?
                .is_none()
            {
                return Ok(None);
            }
            self.conn
                .query_opt(
                    "SELECT 1 FROM legal_text WHERE text_uid = $1 FOR UPDATE",
                    &[&into_uid],
                )
                .await?
                .ok_or_else(|| {
                    StoreError::Invalid(format!("fiche cible de fusion absente: {into_uid}"))
                })?;
            self.absorb_legal_text(from_uid, into_uid).await.map(Some)
        }
        .await;
        match collapsed {
            Ok(counts) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// The end of a collapse, inside its transaction: the identity columns the target lacks,
    /// the slug as a redirect and the incoming links and citations move to `into_uid`; what
    /// `from_uid` still owns goes with it.
    async fn absorb_legal_text(&self, from_uid: &str, into_uid: &str) -> Result<TextRenameCounts> {
        self.conn
            .execute(
                "UPDATE legal_text t SET date_texte = coalesce(t.date_texte, f.date_texte), \
                    date_publi = coalesce(t.date_publi, f.date_publi), \
                    nor = coalesce(t.nor, f.nor), eli = coalesce(t.eli, f.eli), \
                    instrument_key = coalesce(t.instrument_key, f.instrument_key) \
                   FROM legal_text f WHERE t.text_uid = $2 AND f.text_uid = $1",
                &[&from_uid, &into_uid],
            )
            .await?;
        self.stamp_legal_texts(&[into_uid.to_string()]).await?;
        for sql in [
            "UPDATE legal_text_slug_redirect SET text_uid = $2 WHERE text_uid = $1",
            "INSERT INTO legal_text_slug_redirect (ancien_slug, text_uid, jurisdiction) \
             SELECT slug, $2, jurisdiction FROM legal_text WHERE text_uid = $1 AND slug IS NOT NULL \
             ON CONFLICT (ancien_slug, jurisdiction) DO UPDATE SET text_uid = EXCLUDED.text_uid",
        ] {
            self.conn.execute(sql, &[&from_uid, &into_uid]).await?;
        }
        let mut counts = TextRenameCounts::default();
        for (slot, sql) in [
            (
                &mut counts.links_target as &mut u64,
                "UPDATE legal_link SET target_text_uid = $2 WHERE target_text_uid = $1",
            ),
            (
                &mut counts.links_target_uid,
                "UPDATE legal_link SET target_uid = $2 WHERE target_uid = $1",
            ),
            (
                &mut counts.text_cit_ref,
                "UPDATE text_legal_citation SET ref_text_uid = $2 WHERE ref_text_uid = $1",
            ),
        ] {
            *slot = self.conn.execute(sql, &[&from_uid, &into_uid]).await?;
        }
        for (slot, sql) in [
            (
                &mut counts.articles as &mut u64,
                "DELETE FROM legal_article WHERE text_uid = $1",
            ),
            (
                &mut counts.toc,
                "DELETE FROM legal_toc_edge WHERE text_uid = $1 OR owner_uid = $1",
            ),
            (
                &mut counts.links_owner,
                "DELETE FROM legal_link WHERE owner_text_uid = $1",
            ),
        ] {
            *slot = self.conn.execute(sql, &[&from_uid]).await?;
        }
        // Its outgoing citations and usages go with the row: the FKs on `legal_text` cascade.
        self.conn
            .execute("DELETE FROM legal_text WHERE text_uid = $1", &[&from_uid])
            .await?;
        Ok(counts)
    }

    /// Reads what [`Self::merge_legal_text_version`] would write. `None` when `from_uid` is
    /// gone, merged already.
    #[tracing::instrument(name = "db.text_version_merge", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_version_merge(
        &self,
        from_uid: &str,
        into_uid: &str,
    ) -> Result<Option<TextVersionMerge>> {
        let Some(from) = self
            .conn
            .query_opt(
                "SELECT title, slug FROM legal_text WHERE text_uid = $1",
                &[&from_uid],
            )
            .await?
        else {
            return Ok(None);
        };
        let into_slug: Option<String> = self
            .conn
            .query_opt(
                "SELECT slug FROM legal_text WHERE text_uid = $1",
                &[&into_uid],
            )
            .await?
            .and_then(|r| r.get(0));
        let r = self
            .conn
            .query_one(
                "WITH f AS (SELECT num_key, date_debut, date_fin FROM legal_article WHERE text_uid = $1), \
                      i AS (SELECT num_key, min(date_debut) AS first FROM legal_article \
                             WHERE text_uid = $2 GROUP BY num_key), \
                      c AS (SELECT f.date_debut, \
                                   least(f.date_fin, coalesce(i.first, (SELECT min(first) FROM i))) AS fin \
                              FROM f LEFT JOIN i USING (num_key)) \
                 SELECT (SELECT count(*) FROM f), \
                        (SELECT count(*) FROM legal_article WHERE text_uid = $2), \
                        (SELECT count(DISTINCT num_key) FROM f JOIN i USING (num_key)), \
                        (SELECT count(DISTINCT f.num_key) FROM f LEFT JOIN i USING (num_key) \
                          WHERE i.num_key IS NULL), \
                        (SELECT min(first) FROM i), \
                        (SELECT count(*) FROM c WHERE fin IS NULL OR fin <= date_debut), \
                        (SELECT count(*) FROM f JOIN legal_article a ON a.text_uid = $2 \
                            AND a.num_key = f.num_key AND a.date_debut = f.date_debut), \
                        (SELECT count(*) FROM legal_link WHERE owner_text_uid = $1)",
                &[&from_uid, &into_uid],
            )
            .await?;
        Ok(Some(TextVersionMerge {
            from_title: from.get(0),
            from_slug: from.get(1),
            into_slug,
            from_articles: r.get(0),
            into_articles: r.get(1),
            shared_keys: r.get(2),
            from_only_keys: r.get(3),
            close: r.get(4),
            empty_windows: r.get(5),
            collisions: r.get(6),
            owned_links: r.get(7),
        }))
    }

    /// Merges `from_uid`, another reading of the act `into_uid` carries, into it as one of
    /// its versions: each of its articles moves under `into_uid` with what it owns, closed
    /// on the first date `into_uid` writes that article, else the text, then the text
    /// collapses as [`Self::collapse_legal_text`] does, and `aliases` become the names the
    /// target is cited by. Refuses a row whose window would be empty or whose key the target
    /// holds. Idempotent: `from_uid` absent → `None`.
    #[tracing::instrument(name = "db.merge_legal_text_version", skip(self), fields(db.system = "postgresql"))]
    pub async fn merge_legal_text_version(
        &self,
        from_uid: &str,
        into_uid: &str,
        aliases: &[TextAlias],
    ) -> Result<Option<TextRenameCounts>> {
        self.conn.batch_execute("BEGIN").await?;
        let merged: Result<Option<TextRenameCounts>> = async {
            for uid in [from_uid, into_uid] {
                self.conn
                    .query_opt(
                        "SELECT 1 FROM legal_text WHERE text_uid = $1 FOR UPDATE",
                        &[&uid],
                    )
                    .await?;
            }
            let Some(plan) = self.text_version_merge(from_uid, into_uid).await? else {
                return Ok(None);
            };
            if plan.empty_windows > 0 || plan.collisions > 0 {
                return Err(StoreError::Invalid(format!(
                    "{from_uid} into {into_uid}: {} rows with an empty window, {} keys taken",
                    plan.empty_windows, plan.collisions
                )));
            }
            let moved = self
                .conn
                .execute(
                    "UPDATE legal_article a SET text_uid = $2, \
                            date_fin = least(a.date_fin, coalesce( \
                                (SELECT min(o.date_debut) FROM legal_article o \
                                  WHERE o.text_uid = $2 AND o.num_key = a.num_key), \
                                (SELECT min(o.date_debut) FROM legal_article o WHERE o.text_uid = $2))), \
                            status = CASE a.status WHEN 'VIGUEUR' THEN 'MODIFIE' ELSE a.status END \
                      WHERE a.text_uid = $1",
                    &[&from_uid, &into_uid],
                )
                .await?;
            let mut owned = [0u64; 3];
            for (slot, sql) in owned.iter_mut().zip([
                "UPDATE legal_link SET owner_text_uid = $2 \
                  WHERE owner_text_uid = $1 AND owner_num_key <> ''",
                "UPDATE text_legal_citation SET owner_text_uid = $2 \
                  WHERE owner_text_uid = $1 AND owner_num_key IS NOT NULL",
                "UPDATE text_case_citation SET owner_text_uid = $2 \
                  WHERE owner_text_uid = $1 AND owner_num_key IS NOT NULL",
            ]) {
                *slot = self.conn.execute(sql, &[&from_uid, &into_uid]).await?;
            }
            let mut counts = self.absorb_legal_text(from_uid, into_uid).await?;
            self.replace_text_aliases(into_uid, aliases).await?;
            counts.articles = moved;
            [counts.links_owner, counts.text_cit_owner, counts.case_cit] = owned;
            Ok(Some(counts))
        }
        .await;
        match merged {
            Ok(counts) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Supprime une fiche dont l'identité n'a pas d'objet : le véhicule dont elle
    /// est dérivée n'était pas un acte de publication (avis du JO annonçant une
    /// dénonciation, une composition de commission, des contingents d'exportation).
    /// Contrairement à [`Self::collapse_legal_text`], il n'y a pas de fiche jumelle
    /// où faire déménager les entrantes : elles partent avec le reste. Idempotent :
    /// fiche absente → `None`.
    #[tracing::instrument(name = "db.delete_legal_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_legal_text(&self, uid: &str) -> Result<Option<TextRenameCounts>> {
        self.conn.batch_execute("BEGIN").await?;
        let deleted: Result<Option<TextRenameCounts>> = async {
            if self
                .conn
                .query_opt(
                    "SELECT 1 FROM legal_text WHERE text_uid = $1 FOR UPDATE",
                    &[&uid],
                )
                .await?
                .is_none()
            {
                return Ok(None);
            }
            let mut counts = TextRenameCounts::default();
            for (slot, sql) in [
                (
                    &mut counts.articles as &mut u64,
                    "DELETE FROM legal_article WHERE text_uid = $1",
                ),
                (
                    &mut counts.toc,
                    "DELETE FROM legal_toc_edge WHERE text_uid = $1 OR owner_uid = $1",
                ),
                (
                    &mut counts.links_owner,
                    "DELETE FROM legal_link WHERE owner_text_uid = $1",
                ),
                (
                    &mut counts.links_target,
                    "DELETE FROM legal_link WHERE target_text_uid = $1 OR target_uid = $1",
                ),
            ] {
                *slot = self.conn.execute(sql, &[&uid]).await?;
            }
            // Citations texte→texte et usages partent en cascade avec la fiche.
            self.conn
                .execute("DELETE FROM legal_text WHERE text_uid = $1", &[&uid])
                .await?;
            Ok(Some(counts))
        }
        .await;
        match deleted {
            Ok(counts) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }
}
