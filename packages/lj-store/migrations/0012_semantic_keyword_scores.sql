-- Migration 0012 -- Scores alignes pour les keywords semantiques.
--
-- Les tableaux restent compacts et ordonnes; les scores servent au filtrage
-- query-time et a l'affichage sans imposer une table fille par mot.

ALTER TABLE decision_semantic_keywords
ADD COLUMN keyword_scores REAL[] NOT NULL DEFAULT ARRAY[]::REAL[];

ALTER TABLE decision_semantic_keywords
ADD COLUMN expression_scores REAL[] NOT NULL DEFAULT ARRAY[]::REAL[];
