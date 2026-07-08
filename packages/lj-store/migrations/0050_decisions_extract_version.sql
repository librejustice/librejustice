-- Migration 0050 — Version du pipeline d'extraction par décision (ADR 0083).
--
-- ``decisions.extract_version`` : version de ``lj_core::extract`` qui a
-- produit les champs structurés stockés de la décision (constante
-- ``EXTRACT_VERSION``, incrémentée à chaque changement de comportement des
-- extracteurs). Posée par l'ingest et par ``reextract-fields``, qui ne
-- re-parse que les décisions dont la version diffère — reprise après
-- interruption et re-extracts ciblés sans repasse complète.
--
-- NULL = extrait avant le versionnage (pipeline inconnu).

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS extract_version smallint;
