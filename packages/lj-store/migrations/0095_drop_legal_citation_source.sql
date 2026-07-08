-- Migration 0095 — retire `legal_citation.source` : la provenance d'une citation
-- est portée par `extract_version`, comme tous les champs extraits (ADR 0140).
-- < 1000 = recognizer, ≥ 1000 = gold revu. La colonne (ADR 0139) datait d'avant
-- ce modèle et valait 'recognizer' sur 100 % des lignes.

ALTER TABLE legal_citation DROP COLUMN source;
