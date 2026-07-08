-- 0112 : guérison des labels de juridiction (review exhaustive des titres).
--
-- 1. tj35238 : « Tribunal judiciaire de Redon » était un mislabel de la table
--    location Judilibre — 35238 est le code INSEE de Rennes (vérifié sur
--    pièces : 1 269 des 2 000 décisions sondées mentionnent le barreau de
--    Rennes, 27 celui de Redon).
-- 2. tj97209 : label source sans ville (97209 = Fort-de-France).
-- 3. Fusion des codes doublons issus d'extractions anciennes :
--    ta_d_amiens → ta_amiens (« de d Amiens »), caa_versailless →
--    caa_versailles (typo source), ta_la_reunion → ta_reunion.
-- 4. Accents manquants (labels dérivés de sources en capitales).
--
-- `decisions.search_title` (colonne générée) se recalcule seul ; les colonnes
-- dénormalisées de `decision_chunks` sont resynchronisées en fin de script.

UPDATE jurisdiction SET label = 'Tribunal judiciaire de Rennes', city = 'Rennes'
WHERE code = 'tj35238';
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Fort-de-France', city = 'Fort-de-France'
WHERE code = 'tj97209';
UPDATE jurisdiction SET label = 'Tribunal judiciaire d''Oloron-Sainte-Marie', city = 'Oloron-Sainte-Marie'
WHERE code = 'tj64445';
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Châlons-en-Champagne', city = 'Châlons-en-Champagne'
WHERE code = 'tj51108';
UPDATE jurisdiction SET label = 'Tribunal de commerce d''Angoulême', city = 'Angoulême'
WHERE code = 'tcom1601';
UPDATE jurisdiction SET label = 'Tribunal de commerce de Chambéry', city = 'Chambéry'
WHERE code = 'tcom7301';
UPDATE jurisdiction SET label = 'Tribunal de commerce d''Évry', city = 'Évry'
WHERE code = 'tcom7801';
UPDATE jurisdiction SET label = 'Tribunal de commerce de Châlons-en-Champagne', city = 'Châlons-en-Champagne'
WHERE code = 'tcom5101';
UPDATE jurisdiction SET label = 'Tribunal administratif de Nouvelle-Calédonie', city = 'Nouvelle-Calédonie'
WHERE code = 'ta_nouvelle_caledonie';

-- Fusions : rebrancher les décisions sur le code canonique, puis supprimer
-- les entrées orphelines du référentiel.
UPDATE decisions SET jurisdiction_code = 'ta_amiens' WHERE jurisdiction_code = 'ta_d_amiens';
UPDATE decisions SET jurisdiction_code = 'caa_versailles' WHERE jurisdiction_code = 'caa_versailless';
UPDATE decisions SET jurisdiction_code = 'ta_reunion' WHERE jurisdiction_code = 'ta_la_reunion';
DELETE FROM jurisdiction WHERE code IN ('ta_d_amiens', 'caa_versailless', 'ta_la_reunion');

-- Réalignement de decisions.jurisdiction_name (alimente search_title / BM25).
UPDATE decisions d SET jurisdiction_name = j.label
FROM jurisdiction j
WHERE j.code = d.jurisdiction_code
  AND d.jurisdiction_code IN (
    'tj35238', 'tj97209', 'tj64445', 'tj51108',
    'tcom1601', 'tcom7301', 'tcom7801', 'tcom5101',
    'ta_nouvelle_caledonie', 'ta_amiens', 'caa_versailles', 'ta_reunion'
  )
  AND d.jurisdiction_name IS DISTINCT FROM j.label;

-- Resync des colonnes dénormalisées des chunks (BM25 chunks).
UPDATE decision_chunks c
SET jurisdiction_name = d.jurisdiction_name, search_title = d.search_title
FROM decisions d
WHERE d.id = c.decision_id
  AND d.jurisdiction_code IN (
    'tj35238', 'tj97209', 'tj64445', 'tj51108',
    'tcom1601', 'tcom7301', 'tcom7801', 'tcom5101',
    'ta_nouvelle_caledonie', 'ta_amiens', 'caa_versailles', 'ta_reunion'
  )
  AND (c.jurisdiction_name IS DISTINCT FROM d.jurisdiction_name
       OR c.search_title IS DISTINCT FROM d.search_title);
