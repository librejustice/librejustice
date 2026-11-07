//! Chronology links between decisions: one `decision_links` row is (the
//! attacking decision, the link type, the address of the attacked decision,
//! the resolved target when it exists). Written by the ingest (replacement by
//! decision, like the citations), resolved at write time then in batch
//! (`resolve_pending_decision_links`): an arriving decision attracts the
//! pending links that aim at it.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use super::cases::{resolve_case_address_sqls, ResolveScope};
use super::support::address_columns;
use super::types::DecisionLinkRow;
use super::DecisionRepository;
use crate::error::Result;

/// Item d'écriture bulk : `(decision_id, liens)`. `None` = extraction sans
/// couche chronologie (écrit un set vide).
pub type DecisionLinkWriteItem = (i64, Option<Vec<DecisionLinkRow>>);

/// Empreinte d'un lien pour le skip-diff et le dédoublonnage à l'écriture :
/// type + adresse visée, sans l'état de résolution.
type LinkFingerprint = (String, Option<String>, Option<String>, Option<NaiveDate>);

impl DecisionRepository<'_> {
    /// (Ré)écrit les liens de chronologie d'UNE décision — sucre sur
    /// [`Self::replace_decision_links_bulk`].
    pub async fn replace_decision_links(
        &self,
        decision_id: i64,
        links: Option<&[DecisionLinkRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_decision_links_bulk(&[(decision_id, links.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les liens d'un lot de décisions : garde jamais-dégrader
    /// (version > `EXTRACT_VERSION` = révision manuelle, jamais remplacée),
    /// skip des sets `(link_type, adresse)` inchangés, puis DELETE + INSERT
    /// et résolution des cibles fraîchement écrites. Idempotent. Rend les
    /// décisions RÉELLEMENT réécrites, pour que l'appelant date leur page du
    /// jour.
    #[tracing::instrument(name = "db.replace_decision_links_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_decision_links_bulk(
        &self,
        items: &[DecisionLinkWriteItem],
    ) -> Result<Vec<i64>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
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

        // Skip-diff : set (link_type, adresse) identique → aucune écriture.
        let db_rows = self
            .conn
            .query(
                "SELECT decision_id, link_type, target_court, target_docket, target_date
                 FROM decision_links
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_versions::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<LinkFingerprint>> = HashMap::new();
        for row in &db_rows {
            current.entry(row.get(0)).or_default().insert((
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
            ));
        }
        let empty = HashSet::new();
        let changed: Vec<&DecisionLinkWriteItem> = items
            .iter()
            .filter(|(id, links)| {
                if protected.contains(id) {
                    return false;
                }
                current.get(id).unwrap_or(&empty) != &link_set(links.as_deref().unwrap_or(&[]))
            })
            .collect();
        if changed.is_empty() {
            return Ok(Vec::new());
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM decision_links WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let mut decision_ids: Vec<i64> = Vec::new();
        let mut link_types: Vec<String> = Vec::new();
        let mut courts: Vec<Option<String>> = Vec::new();
        let mut dockets: Vec<Option<String>> = Vec::new();
        let mut dates: Vec<Option<NaiveDate>> = Vec::new();
        for (id, links) in &changed {
            // The set carries the uniqueness the table no longer constrains:
            // two occurrences of one address are a single link.
            for (link_type, court, docket, date) in link_set(links.as_deref().unwrap_or(&[])) {
                decision_ids.push(*id);
                link_types.push(link_type);
                courts.push(court);
                dockets.push(docket);
                dates.push(date);
            }
        }
        if !decision_ids.is_empty() {
            self.conn
                .execute(
                    "INSERT INTO decision_links
                       (decision_id, link_type, target_court, target_docket, target_date,
                        target_decision_id, extract_version)
                     SELECT d, t, c, n, j, NULL, $6
                     FROM unnest($1::bigint[], $2::text[], $3::text[], $4::text[], $5::date[])
                          AS src(d, t, c, n, j)",
                    &[
                        &decision_ids,
                        &link_types,
                        &courts,
                        &dockets,
                        &dates,
                        &lj_versions::EXTRACT_VERSION,
                    ],
                )
                .await?;
            for sql in resolve_case_address_sqls("decision_links", ResolveScope::ByDecision) {
                self.conn.execute(sql.as_str(), &[&changed_ids]).await?;
            }
            self.conn
                .execute(
                    resolve_parties_sql(" AND dl.decision_id = ANY($1)").as_str(),
                    &[&changed_ids],
                )
                .await?;
        }
        Ok(changed_ids)
    }

    /// Resolves in batch every pending link whose target has arrived in the
    /// base: the target is the **single active** decision at the address of the
    /// link (several matches mean a serial number, which is never guessed).
    /// Returns how many links were resolved. Called at the end of an ingest and
    /// by the weekly pass.
    #[tracing::instrument(name = "db.resolve_pending_decision_links", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_decision_links(&self) -> Result<u64> {
        let mut n = 0u64;
        for sql in resolve_case_address_sqls("decision_links", ResolveScope::All) {
            n += self.conn.execute(sql.as_str(), &[]).await?;
        }
        n += self
            .conn
            .execute(resolve_parties_sql("").as_str(), &[])
            .await?;
        Ok(n)
    }

    /// Twin of the sweep when a batch of targets arrives (reverse probe): the
    /// pending links one docket of the batch touches, served by the partial GIN
    /// index. Same uniqueness as the sweep. An address without a docket is out
    /// of it, being settled by the parties.
    pub(super) async fn resolve_decision_links_for_arrival(
        &self,
        dockets: &[String],
    ) -> Result<u64> {
        let mut n = 0u64;
        for sql in resolve_case_address_sqls("decision_links", ResolveScope::ByArrival) {
            n += self.conn.execute(sql.as_str(), &[&dockets]).await?;
        }
        Ok(n)
    }
}

/// The links of one decision, deduplicated by (type, address).
fn link_set(links: &[DecisionLinkRow]) -> HashSet<LinkFingerprint> {
    links
        .iter()
        .map(|l| {
            let (court, docket, date) = address_columns(&l.address);
            (
                l.link_type.clone(),
                court.map(str::to_string),
                docket.map(str::to_string),
                date,
            )
        })
        .collect()
}

/// Address without a docket: a cassation names the court and the date of the
/// attacked arrêt, never its RG; the parties settle between the arrêts that
/// court and date return.
///
/// Natural persons stay out of the join: the pseudonym is reassigned per
/// decision, so « Vincent X... » in cassation and « Vincent X... » in appeal
/// are two different people. The length guard alone does not do it, it only
/// drops the two-letter era.
fn resolve_parties_sql(scope: &str) -> String {
    format!(
        "WITH pendant AS (
             SELECT dl.id AS lien, dl.decision_id AS amont,
                    dl.target_court AS cour, dl.target_date AS jour
             FROM decision_links dl
             WHERE dl.target_decision_id IS NULL
               AND dl.target_docket IS NULL
               AND dl.target_court IS NOT NULL
               AND dl.target_date IS NOT NULL{scope}
         ), candidat AS (
             SELECT p.lien, d.id AS cible
             FROM pendant p
             JOIN decisions d
               ON d.jurisdiction_code = p.cour
              AND d.date_lecture = p.jour
              AND d.deleted_at IS NULL
             WHERE EXISTS (
                 SELECT 1 FROM decision_actor a
                 JOIN decision_actor b
                   ON b.decision_id = d.id AND b.quality = 'party'
                  AND b.nature IS DISTINCT FROM 'physique'
                  AND ((a.entity_uid IS NOT NULL AND a.entity_uid = b.entity_uid)
                    OR (length(lj_party_fold(a.resolve_key)) >= 4
                        AND lj_party_fold(a.resolve_key) = lj_party_fold(b.resolve_key)))
                 WHERE a.decision_id = p.amont AND a.quality = 'party'
                   AND a.nature IS DISTINCT FROM 'physique')
         ), tranche AS (
             SELECT lien, min(cible) AS cible FROM candidat
             GROUP BY lien HAVING count(DISTINCT cible) = 1
         )
         UPDATE decision_links dl SET target_decision_id = t.cible
         FROM tranche t WHERE dl.id = t.lien"
    )
}
