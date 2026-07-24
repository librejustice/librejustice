-- Migration 0121 — Cassation ramenée à la maille cour (ADR 0172).
--
-- Le référentiel `jurisdiction` mélangeait deux mailles : la Cassation à la
-- chambre (`cass_civ1`, `cass_soc`… avec la chambre fondue dans le label),
-- tout le reste à la cour. Invariant nouveau : `jurisdiction` = la cour, pour
-- tous les ordres. La chambre vit dans les axes déjà remplis
-- (`chamber_position` fin + `chambre_uid` catégorie, ADR 0170), jamais dans
-- l'identité de juridiction.
--
-- `search_title` (BM25) n'est pas réécrite : « Cour de cassation, chambre
-- sociale, … » et « Cour de cassation » + siège « chambre sociale » tokenisent
-- à l'identique (ponctuation strippée).

-- 1. Repointe toutes les décisions Cassation sur la juridiction unique `cc`.
--    (La ligne `cc` = « Cour de cassation » existe déjà.)
UPDATE decisions SET jurisdiction_code = 'cc'
WHERE juridiction_type = 'CC' AND jurisdiction_code <> 'cc';

-- 2. Supprime les lignes `jurisdiction` de grain-chambre, désormais orphelines
--    (la FK decisions.jurisdiction_code -> jurisdiction(code) est satisfaite :
--    plus aucune décision ne les référence après l'étape 1).
DELETE FROM jurisdiction WHERE code LIKE 'cass\_%';

ANALYZE decisions;
