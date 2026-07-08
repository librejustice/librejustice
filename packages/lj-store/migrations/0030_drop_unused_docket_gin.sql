-- Migration 0030 — Drop de l'index GIN ``idx_decisions_docket_numbers``.
--
-- ``decisions.docket_numbers`` n'est jamais filtré côté API ni côté ingest :
-- audit grep + ``pg_stat_user_indexes.idx_scan = 0``. La colonne est
-- uniquement sélectionnée pour le payload (filename PDF/DOCX, search_title
-- généré, sortie search/decision/MCP). Aucune requête ``WHERE 'X' = ANY(...)``
-- ni ``@>`` ni ``&&``.
--
-- Conséquences positives :
--   - chaque UPDATE sur ``decisions`` cesse de maintenir un GIN (5–10× plus
--     cher qu'un B-tree). Bénéfice diffus sur ingest et reextract-fields.
--   - plus de risque de ``ProgramLimitExceeded`` (clé GIN > 2,7 Ko) ; la
--     garde-fou de longueur posée côté extractor (32 chars max) reste utile
--     pour la propreté des données, mais n'est plus indispensable côté DB.
--   - storage récupéré.
--
-- La colonne ``docket_numbers`` est conservée — elle alimente la génération
-- de ``search_title`` (qui colle ``docket_numbers[1]`` en suffixe).

DROP INDEX IF EXISTS idx_decisions_docket_numbers;
