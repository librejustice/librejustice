-- Migration 0092 — couche GOLD unifiée (vérité-terrain en base, version=1000).
--
-- Porte les ground-truths du bench (jadis fichiers `apps/lj-bench/gt/*`) DANS la
-- base, attachés à leur décision, à `version=1000` (couche gold/LLM distincte
-- d'`EXTRACT_VERSION`). Couvre TOUS les champs d'extraction (outcome, parties,
-- juridiction, domaine, facettes, sections…), pas seulement les citations — les
-- liens de citations gold vivent, eux, dans `legal_citation` (source llm/human,
-- ADR 0139). Ici : un blob JSONB par (décision, kind, version).
--
--   version : 1000 = gold/LLM curé (distinct d'EXTRACT_VERSION=8, le déterministe).
--   kind    : 'extraction' (les 32 champs) | 'sections' | 'facets' | … extensible.
--   payload : l'annotation gold (JSONB), forme libre selon kind.
--   source  : 'llm:<model>' | 'human' | 'swarm'.
--
-- ADDITIVE : ne touche aucune table existante ; les colonnes extraites live de
-- `decisions` (EXTRACT_VERSION) restent la vérité de service. `gold_annotation`
-- est la référence d'évaluation/curation. UNIQUE (decision_id, kind, version) →
-- upsert idempotent.

CREATE TABLE gold_annotation (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id       BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    version           INT  NOT NULL,
    kind              TEXT NOT NULL,
    payload           JSONB NOT NULL,
    source            TEXT NOT NULL,
    review_confidence REAL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (decision_id, kind, version)
);

CREATE INDEX idx_gold_decision ON gold_annotation (decision_id);
