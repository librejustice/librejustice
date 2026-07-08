-- Migration 0031 — ``decision_full_text.source_payload_gzip`` en STORAGE
-- EXTERNAL (au lieu de EXTENDED).
--
-- La colonne stocke des bytes **déjà gzippés** côté Python (gzip niveau 6,
-- ratio ~5-6× sur du texte juridique — meilleur que pglz/lz4 de PG). En
-- ``EXTENDED`` (défaut bytea), PG tente quand même une compression pglz/lz4
-- à chaque INSERT/UPDATE : sur des bytes incompressibles le ratio est ~1.0
-- et PG fallback en clair, donc pas de double compression en stockage —
-- mais le cycle CPU de la tentative est gaspillé à chaque écriture.
--
-- ``EXTERNAL`` = out-of-line dans TOAST, **sans tentative** de compression.
-- N'affecte que les futures écritures ; les rows existants gardent leur
-- format jusqu'à un éventuel ``VACUUM FULL``.

ALTER TABLE decision_full_text
    ALTER COLUMN source_payload_gzip SET STORAGE EXTERNAL;
