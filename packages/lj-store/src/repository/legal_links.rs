//! Arêtes `legal_link` (graphe de liens DILA, ADR 0174) : écriture par
//! **remplacement par propriétaire** (DELETE + COPY binaire, idempotent,
//! rejouable — même patron que `write_citation_occurrences`), lecture résolue
//! au read-time contre `legal_text`/`legal_article`, et purge des orphelins
//! après backfill. Sentinelles owner texte-niveau : `num_key = ''`,
//! `date_debut = '0001-01-01'` (alignées sur `legal_article`).

use chrono::NaiveDate;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

use super::types::{LegalLinkOwner, LegalLinkRow, ResolvedLegalLink};
use super::DecisionRepository;
use crate::error::Result;

/// Sentinelle `owner_date_debut` (même borne ouverte que `legal_article`).
fn owner_date(d: Option<NaiveDate>) -> NaiveDate {
    d.unwrap_or_else(|| NaiveDate::from_ymd_opt(1, 1, 1).expect("0001-01-01"))
}

impl DecisionRepository<'_> {
    /// Remplace les arêtes de chaque propriétaire du batch : un DELETE par
    /// clés (unnest) puis un COPY binaire de toutes les lignes, `seq` = ordre
    /// du `Vec`. Un propriétaire avec un `Vec` vide voit ses arêtes purgées.
    /// Un propriétaire présent plusieurs fois dans le batch (deux fichiers
    /// DILA pour la même identité de version) suit la règle de l'upsert
    /// article : **le dernier gagne** (sinon le COPY viole la PK).
    /// Renvoie le nombre de lignes écrites.
    #[tracing::instrument(name = "db.replace_legal_links", skip_all, fields(db.system = "postgresql", owners = items.len()))]
    pub async fn replace_legal_links(
        &self,
        items: &[(LegalLinkOwner, Vec<LegalLinkRow>)],
    ) -> Result<u64> {
        let mut last: std::collections::HashMap<(&str, &str, NaiveDate), usize> =
            std::collections::HashMap::new();
        for (i, (o, _)) in items.iter().enumerate() {
            last.insert(
                (
                    o.text_uid.as_str(),
                    o.num_key.as_str(),
                    owner_date(o.date_debut),
                ),
                i,
            );
        }
        let items: Vec<&(LegalLinkOwner, Vec<LegalLinkRow>)> = items
            .iter()
            .enumerate()
            .filter(|(i, (o, _))| {
                last[&(
                    o.text_uid.as_str(),
                    o.num_key.as_str(),
                    owner_date(o.date_debut),
                )] == *i
            })
            .map(|(_, item)| item)
            .collect();
        if items.is_empty() {
            return Ok(0);
        }
        let uids: Vec<&str> = items.iter().map(|(o, _)| o.text_uid.as_str()).collect();
        let nums: Vec<&str> = items.iter().map(|(o, _)| o.num_key.as_str()).collect();
        let dates: Vec<NaiveDate> = items
            .iter()
            .map(|(o, _)| owner_date(o.date_debut))
            .collect();
        self.conn
            .execute(
                "DELETE FROM legal_link \
                 WHERE (owner_text_uid, owner_num_key, owner_date_debut) IN \
                       (SELECT * FROM unnest($1::text[], $2::text[], $3::date[]))",
                &[&uids, &nums, &dates],
            )
            .await?;

        let sink = self
            .conn
            .copy_in(
                "COPY legal_link (owner_text_uid, owner_num_key, owner_date_debut, seq, \
                 typelien, verb, direction, target_kind, target_uid, target_text_uid, \
                 target_num, target_num_key, target_nature, target_label, target_date, \
                 target_nor) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::INT4,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::TEXT,
            ],
        );
        tokio::pin!(writer);
        let mut written: u64 = 0;
        for ((owner, rows), date) in items.iter().copied().zip(&dates) {
            for (seq, l) in rows.iter().enumerate() {
                let seq = seq as i32;
                writer
                    .as_mut()
                    .write(&[
                        &owner.text_uid,
                        &owner.num_key,
                        date,
                        &seq,
                        &l.typelien,
                        &l.verb,
                        &l.direction,
                        &l.target_kind,
                        &l.target_uid,
                        &l.target_text_uid,
                        &l.target_num,
                        &l.target_num_key,
                        &l.target_nature,
                        &l.target_label,
                        &l.target_date,
                        &l.target_nor,
                    ])
                    .await?;
                written += 1;
            }
        }
        writer.finish().await?;
        Ok(written)
    }

    /// Arêtes d'une version d'article, cibles résolues : article par
    /// `source_uid` (ID DILA exact) sinon par `(texte, num_key)` ; texte par
    /// `target_text_uid`. Ordre du fichier source (`seq`).
    #[tracing::instrument(name = "db.article_links", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_links(
        &self,
        text_uid: &str,
        num_key: &str,
        date_debut: Option<NaiveDate>,
    ) -> Result<Vec<ResolvedLegalLink>> {
        let date = owner_date(date_debut);
        let rows = self
            .conn
            .query(
                "SELECT l.typelien, l.verb, l.direction, l.target_kind, l.target_num, \
                        l.target_nature, l.target_label, l.target_date, l.target_text_uid, \
                        tt.slug AS target_text_slug, tt.title AS target_text_title, \
                        ra.num_key AS resolved_num_key, rt.slug AS resolved_slug, \
                        ts.child_cid AS resolved_section_cid \
                 FROM legal_link l \
                 LEFT JOIN legal_text tt ON tt.text_uid = l.target_text_uid \
                 LEFT JOIN LATERAL ( \
                     SELECT a.num_key, a.text_uid FROM legal_article a \
                     WHERE l.target_kind = 'article' \
                       AND ((l.target_uid IS NOT NULL AND a.source_uid = l.target_uid) \
                         OR (l.target_uid IS NULL AND l.target_num_key IS NOT NULL \
                             AND a.text_uid = l.target_text_uid \
                             AND a.num_key = l.target_num_key)) \
                     LIMIT 1 \
                 ) ra ON true \
                 LEFT JOIN legal_text rt ON rt.text_uid = ra.text_uid \
                 LEFT JOIN LATERAL ( \
                     SELECT te.child_cid FROM legal_toc_edge te \
                     WHERE l.target_kind = 'section' AND l.target_uid IS NOT NULL \
                       AND te.child_uid = l.target_uid \
                     LIMIT 1 \
                 ) ts ON true \
                 WHERE l.owner_text_uid = $1 AND l.owner_num_key = $2 \
                   AND l.owner_date_debut = $3 \
                 ORDER BY l.seq",
                &[&text_uid, &num_key, &date],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ResolvedLegalLink {
                typelien: r.get("typelien"),
                verb: r.get("verb"),
                direction: r.get("direction"),
                target_kind: r.get("target_kind"),
                target_num: r.get("target_num"),
                target_nature: r.get("target_nature"),
                target_label: r.get("target_label"),
                target_date: r.get("target_date"),
                target_text_uid: r.get("target_text_uid"),
                target_text_slug: r.get("target_text_slug"),
                target_text_title: r.get("target_text_title"),
                resolved_slug: r.get("resolved_slug"),
                resolved_num_key: r.get("resolved_num_key"),
                resolved_section_cid: r.get("resolved_section_cid"),
            })
            .collect())
    }

    /// Purge les arêtes dont le propriétaire n'existe plus (article supprimé
    /// par une liste `.dat` rejouée avant le backfill, texte collapsé…). Passe
    /// de fin de backfill — anti-join pleine table, l'appelant lève le
    /// `statement_timeout`.
    #[tracing::instrument(name = "db.prune_orphan_legal_links", skip(self), fields(db.system = "postgresql"))]
    pub async fn prune_orphan_legal_links(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_link ll \
                 WHERE (ll.owner_num_key <> '' AND NOT EXISTS ( \
                            SELECT 1 FROM legal_article a \
                            WHERE a.text_uid = ll.owner_text_uid \
                              AND a.num_key = ll.owner_num_key \
                              AND a.date_debut = ll.owner_date_debut)) \
                    OR (ll.owner_num_key = '' AND NOT EXISTS ( \
                            SELECT 1 FROM legal_text t \
                            WHERE t.text_uid = ll.owner_text_uid))",
                &[],
            )
            .await?;
        Ok(n)
    }
}
