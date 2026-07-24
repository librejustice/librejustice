-- Migration 0143 — filtres d'historique : clé `jurisdictionType` (ADR 0210).
--
-- `user_search_history.filters` est la projection serde du `SearchRequest`
-- (camelCase). Le renommage 0142 fait écrire `jurisdictionType` ; les lignes
-- antérieures portent `juridictionType`, et une strate plus ancienne
-- `juridiction_type`. On replie les deux graphies héritées sur la clé
-- courante pour que l'affichage des chips (describe_filters) reste uniforme.

UPDATE user_search_history
SET filters = (filters - 'juridictionType' - 'juridiction_type')
              || jsonb_build_object(
                     'jurisdictionType',
                     COALESCE(filters -> 'juridictionType', filters -> 'juridiction_type')
                 )
WHERE filters ?| ARRAY['juridictionType', 'juridiction_type'];
