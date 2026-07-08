-- Migration 0094 — resserre les colonnes d'extraction de 0093 (audit sur 91 gold).
--
-- 0093 avait ajouté 20 colonnes ; l'audit du contenu réel (wave 1 gold) en retient
-- 8 : les 6 parties « avocat / cabinet / entreprise » par camp, la défenderesse
-- administrative, et la matière (`legal_domain`). Les 12 autres tombent :
--
--   * applicant/defendant_individuals   — pseudonymisés (« M. B A ») : valeur nulle.
--   * detailed_main_outcome             — doublon de main_outcome, valeurs incohérentes.
--   * decision_type                     — dérivable d'instance_level + special_procedure.
--   * jurisdiction_location_code/label  — mal défini (« 75 » vs « ca_paris ») /
--                                         dérivable de jurisdiction_name.
--   * publication_scope/status          — doublons de publication_codes (4/91 et 0/91).
--   * search_keywords                   — vocabulaire libre non contrôlé.
--   * intervenors                       — 8/91, shape incohérente (strings vs objets).
--   * challenged_acts, lower_decisions  — objets structurés : une colonne est la
--                                         mauvaise forme (liens décision→décision /
--                                         table dédiée si le besoin se confirme).

ALTER TABLE decisions DROP COLUMN applicant_individuals;
ALTER TABLE decisions DROP COLUMN defendant_individuals;
ALTER TABLE decisions DROP COLUMN intervenors;
ALTER TABLE decisions DROP COLUMN challenged_acts;
ALTER TABLE decisions DROP COLUMN decision_type;
ALTER TABLE decisions DROP COLUMN detailed_main_outcome;
ALTER TABLE decisions DROP COLUMN jurisdiction_location_code;
ALTER TABLE decisions DROP COLUMN jurisdiction_location_label;
ALTER TABLE decisions DROP COLUMN publication_scope;
ALTER TABLE decisions DROP COLUMN publication_status;
ALTER TABLE decisions DROP COLUMN search_keywords;
ALTER TABLE decisions DROP COLUMN lower_decisions;
