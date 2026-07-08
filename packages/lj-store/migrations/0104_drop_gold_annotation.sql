-- Migration 0104 — suppression définitive de `gold_annotation` (ADR 0148 §3
-- ré-amendé) : la 0103 est annulée. Les révisions gold vivent dans les MÊMES
-- colonnes de `decisions` que l'extraction déterministe, à une
-- `extract_version` spécifique (1000, constante du banc) — aucune table
-- dédiée, aucune mécanique gold hors `lj-bench`. Le JSONL d'annotation
-- (versionné en git LFS sous apps/lj-bench/gt/) est la couche vivante ; la
-- base ne reçoit que la projection conforme au schéma.

DROP TABLE IF EXISTS gold_annotation;
