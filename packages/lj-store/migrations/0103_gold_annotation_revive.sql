-- Migration 0103 — recréation de `gold_annotation` (ADR 0148 §3).
--
-- La 0093 avait droppé la table (« gold = colonnes de `decisions` à
-- extract_version=1000 »). L'ADR 0148 la ré-décide pour les golds FACETS :
-- « jamais de gold partiel » interdit le stamp v1000 par colonnes quand seules
-- les facettes sont revues (une décision v1000 est réputée entièrement gold,
-- citations comprises). `gold_annotation` porte la référence d'ÉVALUATION par
-- kind, les colonnes de `decisions` restent à l'extracteur.
--
--   version : 1000 = gold/LLM curé (GOLD_EXTRACT_VERSION).
--   kind    : 'facets' (schéma verrouillé ADR 0149) | … extensible.
--   payload : l'annotation gold (JSONB) au schéma du kind, validée avant load.
--   source  : 'llm:<model>' | 'human' | 'swarm'.

CREATE TABLE gold_annotation (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id       BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    version           INT  NOT NULL,
    kind              TEXT NOT NULL,
    payload           JSONB NOT NULL,
    source            TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (decision_id, kind, version)
);

CREATE INDEX idx_gold_decision ON gold_annotation (decision_id);
