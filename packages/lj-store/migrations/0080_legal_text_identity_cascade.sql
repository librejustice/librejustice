-- Migration 0080 — Identité canonique des instruments : colonnes de cascade (ADR 0115).
--
-- ADR 0112 promettait « une identité par instrument » mais a gardé `text_uid` = uid du
-- diffuseur (LEGITEXT vs JORFTEXT) → 24 889 doublons. On ajoute les clés diffuseur-
-- agnostiques de la cascade `eli → nor → instrument_key` qui permettront le collapse
-- (étape destructive ultérieure, hors de cette migration additive).
--
-- DDL seul, additif et déployable à chaud : trois colonnes nullables (ADD COLUMN sans
-- défaut = métadonnée instantanée en PG, pas de réécriture de table). Le remplissage se
-- fait à l'ingest (parser DILA capture `<ID_ELI>`/`<NOR>`/`<NUM>`, `instrument_key`
-- calculé Rust) ; tant que rien ne les lit, elles ne changent aucun comportement live.
--
--   eli            : <META_COMMUN/ID_ELI>, autoritaire quand présent (~20 %).
--   nor            : <META_COMMUN/NOR>, identité cross-diffuseur (~80 %, workhorse).
--   instrument_key : normalize(nature|date|num), filet pour les actes sans ELI/NOR.

ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS eli            TEXT;
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS nor            TEXT;
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS instrument_key TEXT;

-- Index de collapse : lookup d'une identité existante par chaque tier de la cascade.
-- Partiels (WHERE … IS NOT NULL) : la majorité des lignes n'ont pas chaque clé.
CREATE INDEX IF NOT EXISTS idx_legal_text_eli ON legal_text (eli) WHERE eli IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_legal_text_nor ON legal_text (nor) WHERE nor IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_legal_text_instrument_key
    ON legal_text (instrument_key) WHERE instrument_key IS NOT NULL;
