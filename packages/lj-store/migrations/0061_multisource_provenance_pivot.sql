-- Migration 0061 — Pivot de provenance & identité canonique (ADR 0098), passe
-- ADDITIVE uniquement.
--
-- Finalise la frontière de l'ADR 0098 §2 : `decision_sources` devient le pivot
-- complet de provenance, `decisions` devient le canonique pur. Cette migration
-- ne fait QUE les ajouts (no-op déployable seule, avant tout backfill) :
--
--   * decision_sources += `source_fields jsonb` (payload méta de la provenance,
--     descend de `decisions.source_fields`) + `deleted_at timestamptz` (tombstone
--     PAR provenance : c'est lui qui porte la suppression, §5).
--   * decisions += `identity_key text` (clé d'identité universelle indexée
--     `juridiction|numéro|date`, §1) + `deleted_at timestamptz` (cache dérivé
--     pour le filtre recherche, ré-ajouté — il avait été droppé en 0026 ; §2).
--
-- Le BACKFILL (portage des provenances + calcul `identity_key` + fusion
-- rétroactive des doublons, ADR 0098 §7) se fait HORS-MIGRATION via
-- `lj-ingest dedup-backfill` (batché par keyset, repris), pour ne pas tenir une
-- transaction longue sur ~3M lignes (base low-IOPS).
--
-- Le DROP des colonnes mono-source de `decisions` (`source_uid`,
-- `content_checksum`, `source_fields`) + `decision_links` + `decisions
-- .links_version` est différé à une migration ULTÉRIEURE, une fois les lecteurs
-- portés sur `decision_sources.source_uid` (ADR 0098 Conséquences, ordre staged).

-- ── decision_sources : pivot complet ───────────────────────────────────────
ALTER TABLE decision_sources
    ADD COLUMN IF NOT EXISTS source_fields jsonb,
    ADD COLUMN IF NOT EXISTS deleted_at    timestamptz;

ALTER TABLE decision_sources
    ALTER COLUMN source_fields SET COMPRESSION pglz;

-- Provenances actives d'une décision (filtre par défaut du reconcile / lecture).
CREATE INDEX IF NOT EXISTS idx_decision_sources_active
    ON decision_sources (decision_id)
    WHERE deleted_at IS NULL;

-- ── decisions : canonique pur ──────────────────────────────────────────────
ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS identity_key text,
    ADD COLUMN IF NOT EXISTS deleted_at   timestamptz;

-- Probe d'identité at-ingest : `identity_key` couvre les 88 % sans ECLI (§1).
-- NON unique (deux provenances d'une même décision partagent la clé avant fusion ;
-- l'unicité réelle est portée par la fusion §7, pas par une contrainte).
CREATE INDEX IF NOT EXISTS idx_decisions_identity_key
    ON decisions (identity_key)
    WHERE identity_key IS NOT NULL;

-- Filtre recherche / listings : seules les décisions actives (§5, « vide
-- volontaire »). Index partiel calqué sur les autres facettes (0009).
CREATE INDEX IF NOT EXISTS idx_decisions_active
    ON decisions (id)
    WHERE deleted_at IS NULL;
