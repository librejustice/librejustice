-- Migration 0062 — Retrait du graphe de citations décision↔décision (ADR 0098 §6).
--
-- Le doc-linking passe désormais par les provenances (`decision_sources`) ; le
-- graphe de citations brutes est différé (recomputable depuis `source_fields`
-- jsonb au besoin). Tous les lecteurs/écrivains Rust ont été retirés
-- (`replace_decision_links`, `resolve_pending_links`, `decisions_for_links_backfill`,
-- `set_links_version`, `upsert_ariane_parent_link`, commande `backfill-links`,
-- `extract_decision_links`, DTO `DecisionLink`/`DecisionLinkKind`).
--
-- `DROP TABLE` emporte ses index (`idx_decision_links_*`) et contraintes
-- (`decision_links_pk`, CHECK `decision_links_target_present`). La colonne
-- `decisions.links_version` (watermark du backfill de liens, 0055) part avec.

DROP TABLE IF EXISTS decision_links;

ALTER TABLE decisions DROP COLUMN IF EXISTS links_version;
