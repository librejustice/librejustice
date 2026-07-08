//! Liens de chronologie entre décisions (ADR 0161) : une ligne
//! `decision_links` = (décision qui attaque, type, `canonical_ref` de la
//! décision attaquée, cible résolue quand elle existe). Écrits par l'ingest
//! (remplacement par décision, comme les citations), résolus à l'écriture
//! puis en batch (`resolve_pending_decision_links`) — une décision qui arrive
//! « attire » les liens pendants qui la visent.

use std::collections::{HashMap, HashSet};

use super::types::DecisionLinkRow;
use super::DecisionRepository;
use crate::error::Result;

/// Item d'écriture bulk : `(decision_id, liens)`. `None` = extraction sans
/// couche chronologie (écrit un set vide).
pub type DecisionLinkWriteItem = (i64, Option<Vec<DecisionLinkRow>>);

impl DecisionRepository<'_> {
    /// (Ré)écrit les liens de chronologie d'UNE décision — sucre sur
    /// [`Self::replace_decision_links_bulk`].
    pub async fn replace_decision_links(
        &self,
        decision_id: i64,
        links: Option<&[DecisionLinkRow]>,
    ) -> Result<()> {
        self.replace_decision_links_bulk(&[(decision_id, links.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les liens d'un lot de décisions : garde jamais-dégrader
    /// (version > `EXTRACT_VERSION` = révision manuelle, jamais remplacée),
    /// skip des sets `(link_type, target_ref)` inchangés (la passe intégrale
    /// hebdo ne doit pas churner), puis DELETE + INSERT et résolution des
    /// cibles fraîchement écrites. Idempotent.
    #[tracing::instrument(name = "db.replace_decision_links_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_decision_links_bulk(&self, items: &[DecisionLinkWriteItem]) -> Result<()> {
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

        // Skip-diff : set (link_type, target_ref) identique → aucune écriture.
        let db_rows = self
            .conn
            .query(
                "SELECT decision_id, link_type, target_ref FROM decision_links
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<(String, String)>> = HashMap::new();
        for row in &db_rows {
            current
                .entry(row.get(0))
                .or_default()
                .insert((row.get(1), row.get(2)));
        }
        let empty = HashSet::new();
        let changed: Vec<&DecisionLinkWriteItem> = items
            .iter()
            .filter(|(id, links)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<(String, String)> = links
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|l| (l.link_type.clone(), l.target_ref.clone()))
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(());
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM decision_links WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let mut decision_ids: Vec<i64> = Vec::new();
        let mut link_types: Vec<&str> = Vec::new();
        let mut target_refs: Vec<&str> = Vec::new();
        for (id, links) in &changed {
            for l in links.as_deref().unwrap_or(&[]) {
                decision_ids.push(*id);
                link_types.push(&l.link_type);
                target_refs.push(&l.target_ref);
            }
        }
        if !decision_ids.is_empty() {
            self.conn
                .execute(
                    "INSERT INTO decision_links
                       (decision_id, link_type, target_ref, target_decision_id, extract_version)
                     SELECT d, t, r, NULL, $4
                     FROM unnest($1::bigint[], $2::text[], $3::text[]) AS src(d, t, r)
                     ON CONFLICT (decision_id, link_type, target_ref) DO NOTHING",
                    &[
                        &decision_ids,
                        &link_types,
                        &target_refs,
                        &lj_core::EXTRACT_VERSION,
                    ],
                )
                .await?;
            self.conn
                .execute(RESOLVE_SQL_FOR_DECISIONS, &[&changed_ids])
                .await?;
        }
        Ok(())
    }

    /// Résout en batch tous les liens pendants dont la cible est arrivée en
    /// base : cible = décision **unique et active** au `canonical_ref` du lien
    /// (match multiple = sérielle, on ne devine pas). Renvoie le nombre de
    /// liens résolus. À appeler en fin d'ingest et par la passe hebdo.
    #[tracing::instrument(name = "db.resolve_pending_decision_links", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_decision_links(&self) -> Result<u64> {
        let n = self.conn.execute(RESOLVE_SQL_ALL, &[]).await?;
        Ok(n)
    }
}

const RESOLVE_SQL_FOR_DECISIONS: &str = "
    UPDATE decision_links dl
    SET target_decision_id = (
        SELECT min(d.id) FROM decisions d
        WHERE d.canonical_ref = dl.target_ref AND d.deleted_at IS NULL
        HAVING count(*) = 1)
    WHERE dl.decision_id = ANY($1) AND dl.target_decision_id IS NULL";

const RESOLVE_SQL_ALL: &str = "
    UPDATE decision_links dl
    SET target_decision_id = m.id
    FROM (
        SELECT p.id AS link_id,
               (SELECT min(d.id) FROM decisions d
                WHERE d.canonical_ref = p.target_ref AND d.deleted_at IS NULL
                HAVING count(*) = 1) AS id
        FROM decision_links p
        WHERE p.target_decision_id IS NULL
    ) m
    WHERE dl.id = m.link_id AND m.id IS NOT NULL";
