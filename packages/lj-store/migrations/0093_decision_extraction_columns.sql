-- Migration 0093 — colonnes d'extraction enrichies sur `decisions` + drop gold_annotation.
--
-- Les champs extraits (parties, qualif, différenciateurs analytics) vivent en
-- COLONNES sur `decisions`, comme le reste de l'extraction — pas dans une table
-- gold séparée. Le gold LLM = ces mêmes colonnes écrites à `extract_version=1000`.
-- On abandonne donc `gold_annotation` (migration 0092, JSONB fourre-tout) : mauvaise
-- abstraction.
--
-- Set = UNIQUEMENT les champs RÉELLEMENT extraits (les 32 du swarm) pas encore
-- matérialisés en colonne. On n'ajoute PAS de colonnes spéculatives non extraites
-- ni mal définies (quantum, sens_par_partie, groupes de sociétés) : pas de colonne
-- sans extracteur derrière. La priorité opérateur (avocat/cabinet/entreprise par
-- partie) est couverte par les colonnes parties ci-dessous.
--
-- `rapporteur_public` (extrait par le swarm) est volontairement EXCLU : champ
-- magistrat à faible valeur de recherche et sensible (art. L.111-13 COJ interdit
-- tout profilage). À rajouter seulement si un besoin descriptif clair émerge.
--
-- ADDITIVE : `ADD COLUMN … NULL` est metadata-only (zéro rewrite). Tout NULL tant
-- que non peuplé → comportement inchangé.

-- Parties (priorité opérateur : avocat / cabinet / entreprise, pour chaque camp).
ALTER TABLE decisions ADD COLUMN applicant_companies      TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_individuals    TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_counsel_names  TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_law_firms      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_companies      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_individuals    TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_counsel_names  TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_law_firms      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_administration TEXT;
ALTER TABLE decisions ADD COLUMN intervenors             TEXT[];
ALTER TABLE decisions ADD COLUMN challenged_acts         TEXT[];

-- Qualification (déjà annotée par le swarm, jamais matérialisée en colonne).
ALTER TABLE decisions ADD COLUMN decision_type              TEXT;
ALTER TABLE decisions ADD COLUMN detailed_main_outcome      TEXT;
ALTER TABLE decisions ADD COLUMN legal_domain               TEXT;
ALTER TABLE decisions ADD COLUMN jurisdiction_location_code  TEXT;
ALTER TABLE decisions ADD COLUMN jurisdiction_location_label TEXT;
ALTER TABLE decisions ADD COLUMN publication_scope          TEXT;
ALTER TABLE decisions ADD COLUMN publication_status         TEXT;
ALTER TABLE decisions ADD COLUMN search_keywords            TEXT[];
ALTER TABLE decisions ADD COLUMN lower_decisions            JSONB;

-- Abandon de la couche gold JSONB (0092) : le gold vit dans les colonnes ci-dessus
-- à extract_version=1000.
DROP TABLE IF EXISTS gold_annotation;
