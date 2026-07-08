-- Migration 0004 — Identifiant public opaque pour les décisions

ALTER TABLE decisions
ADD COLUMN IF NOT EXISTS public_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_public_id
ON decisions (public_id)
WHERE public_id IS NOT NULL;
