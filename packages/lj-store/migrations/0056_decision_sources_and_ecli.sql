-- Migration 0056 — Identité inter-sources & déduplication ECLI (ADR 0080).
--
-- DDL bon marché uniquement. Le backfill des provenances existantes (une ligne
-- `decision_sources` par `decisions`, ~3M lignes) se fait HORS-MIGRATION via
-- `lj-ingest backfill-decision-sources` (batché par keyset, repris), pour ne pas
-- tenir une transaction longue qui bloquerait les écritures sur `decisions`
-- (base low-IOPS).
--
-- VERSION ADDITIVE STAGED : `decisions.source_uid` reste en place (NOT NULL
-- UNIQUE) et reste le pivot de l'idempotence intra-source ; `decision_full_text
-- .payload_format` n'est PAS droppé. La table `decision_sources` est ajoutée
-- À CÔTÉ et duplique la provenance, sans rien retirer de l'existant (0089 /
-- reverses / tombstones / find_existing_source_uids lisent encore
-- `decisions.source_uid`). La « migration de source_uid vers decision_sources »
-- décrite par l'ADR est l'état CIBLE, différée à un ADR ultérieur.

-- ── ECLI sur decisions ────────────────────────────────────────────────────
-- Colonne nullable (vide pour l'existant, peuplée au prochain re-extract / par
-- les nouveaux parsers). Aucune dérivation : valeur recopiée verbatim de la
-- source. Index partiel unique calqué sur `idx_decisions_public_id` (0004).
ALTER TABLE decisions
ADD COLUMN IF NOT EXISTS ecli TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;

-- ── Provenances découplées ─────────────────────────────────────────────────
-- Une ligne = une provenance d'une décision canonique. `source_uid UNIQUE` y
-- est dupliqué (l'idempotence intra-source reste portée par decisions.source_uid
-- en parallèle dans cette version staged). `payload_format` étend le CHECK
-- ('xml','json') de 0019 à ('xml','json','dila-xml','html') — prêt pour les
-- parsers DILA/européens (ADRs 0093-0094) sans nouvelle migration.
CREATE TABLE decision_sources (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    source           TEXT     NOT NULL,   -- 'judilibre' | 'opendata' | 'dila-jade' | …
    source_uid       TEXT     NOT NULL UNIQUE,
    content_checksum TEXT     NOT NULL,   -- xxh3-64 du payload brut de CETTE provenance
    source_rank      SMALLINT NOT NULL,   -- gagnant = max (table de rang : lj-store)
    payload_format   TEXT     NOT NULL CHECK (payload_format IN ('xml','json','dila-xml','html')),
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decision_sources_decision_id ON decision_sources (decision_id);
