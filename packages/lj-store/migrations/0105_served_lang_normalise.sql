-- ADR 0153 — langue servie normée : colonne `served_lang` sur `decision_sources`,
-- source unique de vérité de la langue de rendition. Elle remplace le doublon
-- éparpillé `source_fields->>'languageisocode'` (CEDH, verbatim HUDOC 'FRE'/'ENG')
-- vs `source_fields->>'resource_obtained_language'` (CJUE, langue négociée
-- 'fra'/'eng') ET la fonction `lang_rank` (ADR 0127) qui factorisait la définition
-- de « FR » : tout consommateur lit désormais `served_lang`, aucun ne (re)connaît
-- les clés brutes par source — c'est cette divergence qui avait causé le bug de
-- `find_fr_source_uids` (n'testait que la clé CJUE, aveugle à la CEDH).
--
-- VIRTUAL (PG18) : calcul à la lecture, ADD COLUMN métadonnée-seul → PAS de
-- réécriture des 3,7 M lignes / 4,3 GB ni de lock (une colonne STORED forcerait
-- une réécriture complète). Aucun index requis : les deux requêtes qui la lisent
-- pré-filtrent par `decision_id` / `source_uid` (indexés) et ne l'évaluent que sur
-- une poignée de lignes. Les clés brutes restent en `source_fields` (fidélité de
-- provenance) ; `served_lang` est la couche normalisée. Valeurs ISO-639-2/T.
ALTER TABLE decision_sources ADD COLUMN served_lang text
    GENERATED ALWAYS AS (
        CASE
            WHEN source_fields->>'languageisocode' = 'FRE'
              OR source_fields->>'resource_obtained_language' = 'fra' THEN 'fra'
            WHEN source_fields->>'languageisocode' = 'ENG'
              OR source_fields->>'resource_obtained_language' = 'eng' THEN 'eng'
        END
    ) VIRTUAL;

-- `lang_rank` devient redondante : l'ordre d'autorité FR-prioritaire s'exprime
-- directement `(served_lang = 'fra') IS TRUE DESC` (même sémantique : 'fra' en
-- tête, {'eng', NULL} départagés par id). Un seul concept, un seul chemin.
DROP FUNCTION lang_rank(jsonb);
