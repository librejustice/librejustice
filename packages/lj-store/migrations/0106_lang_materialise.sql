-- ADR 0153 — la langue de rendition d'une provenance devient une colonne
-- **matérialisée** simple `decision_sources.lang`, écrite explicitement par le
-- code d'ingest (CEDH/CJUE posent la langue ; `upsert_decision_source` la stocke).
-- Remplace la colonne générée VIRTUAL `served_lang` d'0105 : valeur **stockée**,
-- pas recalculée à la lecture — c'est un fait de la source, pas une dérivation.
ALTER TABLE decision_sources DROP COLUMN served_lang;
ALTER TABLE decision_sources ADD COLUMN lang text;

-- Backfill unique des lignes déjà en base (les ingesters écrivent `lang`
-- directement au-delà). Seule cette migration connaît les clés brutes par source
-- (CEDH `languageisocode` 'FRE'/'ENG' ; CJUE `resource_obtained_language`
-- 'fra'/'eng') ; en aval `lang` est la source unique de vérité. ISO-639-2/T.
UPDATE decision_sources SET lang = CASE
    WHEN source_fields->>'languageisocode' = 'FRE'
      OR source_fields->>'resource_obtained_language' = 'fra' THEN 'fra'
    WHEN source_fields->>'languageisocode' = 'ENG'
      OR source_fields->>'resource_obtained_language' = 'eng' THEN 'eng'
END
WHERE source IN ('cedh', 'cjue');
