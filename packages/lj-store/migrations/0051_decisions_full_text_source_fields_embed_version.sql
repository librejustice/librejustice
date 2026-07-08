-- Migration 0051 — Modèle document quasi-bijectif (ADR 0085) : colonnes
-- ``full_text`` / ``source_fields`` / ``embed_version`` sur ``decisions``.
--
-- ``source_payload ⟷ (full_text, source_fields)`` : le texte nettoyé vit dans
-- ``full_text`` (indexé BM25 au grain décision, ADR 0084) ; tout le reste du
-- payload source vit dans ``source_fields`` (JSONB, offsets rebasés sur
-- ``full_text``). Le payload brut gzippé devient reconstructible par
-- recombinaison et sera dropé (``decision_full_text``) une fois les gates banc
-- passées — étape ultérieure, hors de cette migration.
--
-- ``embed_version`` versionne la génération (chunker + modèle) des embeddings
-- stockés, comme ``extract_version`` (ADR 0083) versionne l'extraction de
-- champs : un futur ``reembed`` ne retouche que les décisions dont la version
-- diffère. NULL = embeddé avant le versionnage.
--
-- Toutes les colonnes sont nullables → migration **no-op** déployable seule,
-- avant tout backfill ou changement de code. ``full_text`` en compression
-- **pglz** (pas lz4) : meilleur ratio sur le texte juridique → moins de pages
-- TOAST → moins d'IOPS en lecture (facteur limitant du mono-serveur).
-- ``source_fields`` garde la compression par défaut (pglz).

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS full_text     text,
    ADD COLUMN IF NOT EXISTS source_fields jsonb,
    ADD COLUMN IF NOT EXISTS embed_version smallint;

ALTER TABLE decisions
    ALTER COLUMN full_text SET COMPRESSION pglz;
