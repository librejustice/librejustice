-- Migration 0100 — référentiels de facettes en base (ADR 0146).
--
-- Modèle « lien » intégral : une table unique `facet_value` à uid namespacé
-- (`domaine:CIVIL_DROIT_LOCATIF`, `solution:REJET`, …) avec vraie FK Postgres,
-- et une table `jurisdiction` (grain = unité juridictionnelle la plus fine
-- utile : ville pour les cours territoriales, chambre pour la Cassation, la
-- juridiction elle-même pour les uniques). Les labels FR vivent ICI, plus dans
-- le code (les trois fichiers labels.rs sont supprimés par la suite du chantier).
--
-- Vocabulaires seedés (arbitrés 2026-07-02/03, ADR 0146 §2) :
--   domaine     : arbre de référence verbatim — 9 racines + 36 feuilles.
--   solution    : 17 valeurs — 15 de référence + Satisfaction totale/partielle.
--   publication : les 6 valeurs de référence (mapping depuis nos 12 codes source).
--   voie        : voie procédurale (badge requêtable, jamais une facette).
--   office      : juge spécialisé (badge + promotion dans la facette juridiction).
--   instance    : stade procédural (hors rail, dérivable, gardé en donnée).
--
-- `jurisdiction` n'est PAS seedée ici : son contenu (villes Judilibre depuis
-- `canonical_ref`, listes TA/CAA, chambres CASS) vient de la donnée d'ingest —
-- backfill CLI dédié, pas du DDL versionné.
--
-- ADDITIVE : colonnes `_uid` nullables sur `decisions`, backfill puis drop des
-- colonnes TEXT libres (`legal_domain`, `main_outcome`, `special_procedure`,
-- `jurisdiction_name`, `formation_or_chamber`, `instance_level`) dans une
-- migration ultérieure, une fois les extracteurs migrés.

CREATE TABLE facet_value (
    uid        TEXT PRIMARY KEY,
    facet      TEXT NOT NULL,
    label      TEXT NOT NULL,
    abbr       TEXT,
    parent_uid TEXT REFERENCES facet_value(uid),
    sort       INT NOT NULL DEFAULT 0,
    CHECK (uid LIKE facet || ':%')
);

CREATE TABLE jurisdiction (
    code             TEXT PRIMARY KEY,
    juridiction_type TEXT NOT NULL,
    city             TEXT,
    label            TEXT NOT NULL
);

ALTER TABLE decisions
    ADD COLUMN legal_domain_uid  TEXT REFERENCES facet_value(uid)
                                 CHECK (legal_domain_uid LIKE 'domaine:%'),
    ADD COLUMN solution_uid      TEXT REFERENCES facet_value(uid)
                                 CHECK (solution_uid LIKE 'solution:%'),
    ADD COLUMN publication_uid   TEXT REFERENCES facet_value(uid)
                                 CHECK (publication_uid LIKE 'publication:%'),
    ADD COLUMN instance_uid      TEXT REFERENCES facet_value(uid)
                                 CHECK (instance_uid LIKE 'instance:%'),
    ADD COLUMN voie_uid          TEXT REFERENCES facet_value(uid)
                                 CHECK (voie_uid LIKE 'voie:%'),
    ADD COLUMN office_uid        TEXT REFERENCES facet_value(uid)
                                 CHECK (office_uid LIKE 'office:%'),
    ADD COLUMN jurisdiction_code TEXT REFERENCES jurisdiction(code);

-- ============================================================
-- domaine — arbre de référence verbatim (9 racines, 36 feuilles).
-- Clé annotée = la feuille ; racine annotée directement quand elle n'a pas de
-- feuille (Fiscal, Européen, Criminel, Constitutionnel) ou quand aucune
-- feuille ne convient.
-- ============================================================

INSERT INTO facet_value (uid, facet, label, parent_uid, sort) VALUES
    ('domaine:CIVIL',                    'domaine', 'Civil',                    NULL, 1),
    ('domaine:COMMERCIAL',               'domaine', 'Commercial',               NULL, 2),
    ('domaine:PUBLIC',                   'domaine', 'Public',                   NULL, 3),
    ('domaine:SOCIAL',                   'domaine', 'Social',                   NULL, 4),
    ('domaine:FISCAL',                   'domaine', 'Fiscal',                   NULL, 5),
    ('domaine:PROPRIETE_INTELLECTUELLE', 'domaine', 'Propriété intellectuelle', NULL, 6),
    ('domaine:EUROPEEN',                 'domaine', 'Européen',                 NULL, 7),
    ('domaine:CRIMINEL',                 'domaine', 'Criminel',                 NULL, 8),
    ('domaine:CONSTITUTIONNEL',          'domaine', 'Constitutionnel',          NULL, 9);

INSERT INTO facet_value (uid, facet, label, parent_uid, sort) VALUES
    ('domaine:CIVIL_PROCEDURES_CIVILES_EXECUTION',           'domaine', 'Procédures civiles d''exécution',                          'domaine:CIVIL', 1),
    ('domaine:CIVIL_DROIT_IMMOBILIER_CONSTRUCTION',          'domaine', 'Droit immobilier et de la construction',                   'domaine:CIVIL', 2),
    ('domaine:CIVIL_DROIT_LOCATIF',                          'domaine', 'Droit locatif',                                            'domaine:CIVIL', 3),
    ('domaine:CIVIL_DROIT_PERSONNES_FAMILLE',                'domaine', 'Droit des personnes et de la famille',                     'domaine:CIVIL', 4),
    ('domaine:CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE','domaine', 'Droit de la copropriété et de la propriété immobilière',   'domaine:CIVIL', 5),
    ('domaine:CIVIL_DROIT_ASSURANCES',                       'domaine', 'Droit des assurances',                                     'domaine:CIVIL', 6),
    ('domaine:CIVIL_DROIT_RESPONSABILITE',                   'domaine', 'Droit de la responsabilité',                               'domaine:CIVIL', 7),
    ('domaine:CIVIL_DROIT_BANCAIRE_BOURSIER',                'domaine', 'Droit bancaire et boursier',                               'domaine:CIVIL', 8),
    ('domaine:CIVIL_DROIT_SUCCESSIONS',                      'domaine', 'Droit des successions',                                    'domaine:CIVIL', 9),
    ('domaine:CIVIL_DROIT_EXPROPRIATION_PREEMPTION',         'domaine', 'Droit de l''expropriation et de préemption',               'domaine:CIVIL', 10),
    ('domaine:CIVIL_DIVORCE_SEPARATION_CORPS',               'domaine', 'Divorce et séparation de corps',                           'domaine:CIVIL', 11),
    ('domaine:CIVIL_DROIT_RURAL',                            'domaine', 'Droit rural',                                              'domaine:CIVIL', 12),
    ('domaine:CIVIL_DROIT_RESPONSABILITE_CONTRATS',          'domaine', 'Droit de la responsabilité et des contrats',               'domaine:CIVIL', 13),
    ('domaine:CIVIL_DROIT_SAISIE_IMMOBILIERE',               'domaine', 'Droit de la saisie immobilière',                           'domaine:CIVIL', 14),
    ('domaine:CIVIL_DROIT_MINEURS',                          'domaine', 'Droit des mineurs',                                        'domaine:CIVIL', 15),
    ('domaine:COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE',      'domaine', 'Droit des entreprises en difficulté',                      'domaine:COMMERCIAL', 1),
    ('domaine:COMMERCIAL_DROIT_BANCAIRE_BOURSIER',           'domaine', 'Droit bancaire et boursier',                               'domaine:COMMERCIAL', 2),
    ('domaine:COMMERCIAL_DROIT_CONTRATS',                    'domaine', 'Droit des contrats',                                       'domaine:COMMERCIAL', 3),
    ('domaine:COMMERCIAL_DROIT_SOCIETES',                    'domaine', 'Droit des sociétés',                                       'domaine:COMMERCIAL', 4),
    ('domaine:COMMERCIAL_DROIT_NUMERIQUE',                   'domaine', 'Droit du numérique',                                       'domaine:COMMERCIAL', 5),
    ('domaine:COMMERCIAL_DROIT_TRANSPORT',                   'domaine', 'Droit du transport',                                       'domaine:COMMERCIAL', 6),
    ('domaine:COMMERCIAL_DROIT_ASSURANCES',                  'domaine', 'Droit des assurances',                                     'domaine:COMMERCIAL', 7),
    ('domaine:COMMERCIAL_DROIT_CONCURRENCE',                 'domaine', 'Droit de la concurrence',                                  'domaine:COMMERCIAL', 8),
    ('domaine:COMMERCIAL_DROIT_CONSOMMATION',                'domaine', 'Droit de la consommation',                                 'domaine:COMMERCIAL', 9),
    ('domaine:COMMERCIAL_DROIT_ARBITRAGE',                   'domaine', 'Droit de l''arbitrage',                                    'domaine:COMMERCIAL', 10),
    ('domaine:PUBLIC_DROIT_ETRANGERS_NATIONALITE',           'domaine', 'Droit des étrangers et de la nationalité',                 'domaine:PUBLIC', 1),
    ('domaine:PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC',     'domaine', 'Droit de l''urbanisme et de l''immobilier public',         'domaine:PUBLIC', 2),
    ('domaine:PUBLIC_DROIT_TRAVAIL',                         'domaine', 'Droit public du travail',                                  'domaine:PUBLIC', 3),
    ('domaine:PUBLIC_DROIT_PENAL_PUBLIC',                    'domaine', 'Droit pénal public',                                       'domaine:PUBLIC', 4),
    ('domaine:PUBLIC_DROIT_AIDE_ACTION_SOCIALE',             'domaine', 'Droit de l''aide et de l''action sociale',                 'domaine:PUBLIC', 5),
    ('domaine:PUBLIC_DROIT_ENVIRONNEMENT',                   'domaine', 'Droit de l''environnement',                                'domaine:PUBLIC', 6),
    ('domaine:SOCIAL_DROIT_TRAVAIL',                         'domaine', 'Droit du travail',                                         'domaine:SOCIAL', 1),
    ('domaine:SOCIAL_DROIT_AIDE_ACTION_SOCIALE',             'domaine', 'Droit de l''aide et de l''action sociale',                 'domaine:SOCIAL', 2),
    ('domaine:SOCIAL_DROIT_PENAL_SOCIAL',                    'domaine', 'Droit pénal social',                                       'domaine:SOCIAL', 3),
    ('domaine:PROPRIETE_INTELLECTUELLE_INDUSTRIELLE',        'domaine', 'Propriété industrielle',                                   'domaine:PROPRIETE_INTELLECTUELLE', 1),
    ('domaine:PROPRIETE_INTELLECTUELLE_LITTERAIRE_ARTISTIQUE','domaine','Propriété littéraire et artistique',                       'domaine:PROPRIETE_INTELLECTUELLE', 2);

-- ============================================================
-- solution — 15 de référence + les 2 satisfactions supplémentaires (1ʳᵉ instance civile).
-- ============================================================

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('solution:REJET',                  'solution', 'Rejet',                  1),
    ('solution:IRRECEVABILITE',         'solution', 'Irrecevabilité',         2),
    ('solution:DESISTEMENT',            'solution', 'Désistement',            3),
    ('solution:NON_LIEU_A_STATUER',     'solution', 'Non-lieu à statuer',     4),
    ('solution:CONFIRMATION',           'solution', 'Confirmation',           5),
    ('solution:INFIRMATION',            'solution', 'Infirmation',            6),
    ('solution:INFIRMATION_PARTIELLE',  'solution', 'Infirmation partielle',  7),
    ('solution:REFORMATION',            'solution', 'Réformation',            8),
    ('solution:CASSATION',              'solution', 'Cassation',              9),
    ('solution:CASSATION_PARTIELLE',    'solution', 'Cassation partielle',    10),
    ('solution:ANNULATION',             'solution', 'Annulation',             11),
    ('solution:CONFORMITE',             'solution', 'Conformité',             12),
    ('solution:NON_CONFORMITE',         'solution', 'Non conformité',         13),
    ('solution:INELIGIBILITE',          'solution', 'Inéligibilité',          14),
    ('solution:SATISFACTION_TOTALE',    'solution', 'Satisfaction totale',    15),
    ('solution:SATISFACTION_PARTIELLE', 'solution', 'Satisfaction partielle', 16),
    ('solution:AUTRE',                  'solution', 'Autre',                  17);

-- ============================================================
-- publication — les 6 valeurs de référence (mapping depuis les codes source).
-- La portée (majeure/importante/limitée, lj-core/publication.rs) n'est pas
-- une facette : affichage seulement.
-- ============================================================

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('publication:PUBLIE_BULLETIN',   'publication', 'Publié au bulletin',                     1),
    ('publication:INEDIT_BULLETIN',   'publication', 'Inédit au bulletin',                     2),
    ('publication:PUBLIE_LEBON',      'publication', 'Publié au recueil Lebon',                3),
    ('publication:MENTIONNE_LEBON',   'publication', 'Mentionné aux tables du recueil Lebon',  4),
    ('publication:INEDIT_LEBON',      'publication', 'Inédit au recueil Lebon',                5),
    ('publication:AUTRE',             'publication', 'Autre',                                  6);

-- ============================================================
-- voie — voie procédurale (issue de la décomposition de special_procedure-25).
-- Badge requêtable, jamais une facette. Vocabulaire fermé, NULL = ordinaire.
-- ============================================================

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('voie:REFERE_SUSPENSION',            'voie', 'Référé-suspension',                              NULL,   1),
    ('voie:REFERE_LIBERTE',               'voie', 'Référé-liberté',                                 NULL,   2),
    ('voie:REFERE_MESURES_UTILES',        'voie', 'Référé mesures utiles',                          NULL,   3),
    ('voie:REFERE_PRECONTRACTUEL',        'voie', 'Référé précontractuel',                          NULL,   4),
    ('voie:REFERE_PROVISION',             'voie', 'Référé-provision',                               NULL,   5),
    ('voie:REFERE_CIVIL',                 'voie', 'Référé civil',                                   NULL,   6),
    ('voie:FILTRAGE_R222_1',              'voie', 'Ordonnance de tri (R. 222-1 CJA)',               NULL,   7),
    ('voie:PAPC',                         'voie', 'Procédure d''admission des pourvois en cassation','PAPC', 8),
    ('voie:QPC',                          'voie', 'Question prioritaire de constitutionnalité',      'QPC',  9),
    ('voie:QUESTION_PREJUDICIELLE_CJUE',  'voie', 'Question préjudicielle à la CJUE',               NULL,   10),
    ('voie:RECOURS_REVISION',             'voie', 'Recours en révision',                            NULL,   11),
    ('voie:TIERCE_OPPOSITION',            'voie', 'Tierce opposition',                              NULL,   12),
    ('voie:RECTIFICATION_INTERPRETATION', 'voie', 'Rectification ou interprétation',                NULL,   13);

-- ============================================================
-- office — juge spécialisé (décomposition de special_procedure-25).
-- Badge + promotion en racine de la facette juridiction (UX de référence).
-- Le sigle vit dans `abbr`, jamais dans un titre.
-- ============================================================

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('office:JLD',               'office', 'Juge des libertés et de la détention',  'JLD', 1),
    ('office:JAF',               'office', 'Juge aux affaires familiales',          'JAF', 2),
    ('office:JCP',               'office', 'Juge des contentieux de la protection', 'JCP', 3),
    ('office:JEX',               'office', 'Juge de l''exécution',                  'JEX', 4),
    ('office:JUGE_ENFANTS',      'office', 'Juge des enfants',                      NULL,  5),
    ('office:PREMIER_PRESIDENT', 'office', 'Premier président',                     NULL,  6),
    ('office:MAGISTRAT_DESIGNE', 'office', 'Magistrat désigné',                     NULL,  7);

-- ============================================================
-- instance — stade procédural (hors rail, gardé en donnée).
-- ============================================================

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('instance:PREMIERE_INSTANCE',          'instance', 'Première instance',          1),
    ('instance:APPEL',                      'instance', 'Appel',                      2),
    ('instance:CASSATION',                  'instance', 'Cassation',                  3),
    ('instance:PREMIER_ET_DERNIER_RESSORT', 'instance', 'Premier et dernier ressort', 4),
    ('instance:RENVOI_APRES_CASSATION',     'instance', 'Renvoi après cassation',     5),
    ('instance:AUTRE',                      'instance', 'Autre',                      6);
