-- Migration 0128 — retrait de `office:JUGE_REFERES` du référentiel.
--
-- Le référé est une voie (`voie:REFERE_*`), jamais un office : depuis
-- EXTRACT_VERSION 35 l'extraction n'émet plus ce uid (la surface greffe
-- « juge des référés » reste un signal interne du parseur — implication
-- `formation:JUGE_UNIQUE`, neutralisée avant sortie) et plus aucune décision
-- ne le porte. La ligne seedée en 0117 est inerte ; le FK
-- `decisions.office_uid → facet_value(uid)` garantit l'échec franc si une
-- décision le portait encore.

DELETE FROM facet_value WHERE uid = 'office:JUGE_REFERES';
