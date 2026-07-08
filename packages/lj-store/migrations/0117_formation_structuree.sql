-- Migration 0117 — formation structurée (ADR 0170).
--
-- `formation_or_chamber` (TEXT libre) est remplacée par trois axes : la
-- position recomposée affichable (`chamber_position`), la spécialisation
-- (`chambre_uid`) et le type de formation (`formation_uid`) — le rôle lu dans
-- la formation greffe alimente les axes `office`/`voie` existants. Les seeds
-- reprennent verbatim les tables du parseur (`lj-extract/src/formation.rs`,
-- CHAMBRE_SEED / FORMATION_SEED / OFFICE_SEED_EXTRA). Le drop de l'ancienne
-- colonne arrive en migration séparée, après backfill et bascule des
-- consommateurs (séquencement ADR 0170).

ALTER TABLE decisions
  ADD COLUMN chamber_position TEXT,
  ADD COLUMN chambre_uid   TEXT REFERENCES facet_value(uid),
  ADD COLUMN formation_uid TEXT REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('chambre:CIVILE',                'chambre', 'Chambre civile',            1),
    ('chambre:SOCIALE',               'chambre', 'Chambre sociale',           2),
    ('chambre:COMMERCIALE',           'chambre', 'Chambre commerciale',       3),
    ('chambre:CRIMINELLE',            'chambre', 'Chambre criminelle',        4),
    ('chambre:CORRECTIONNELLE',       'chambre', 'Chambre correctionnelle',   5),
    ('chambre:PRUD_HOMALE',           'chambre', 'Chambre prud''homale',      6),
    ('chambre:PROTECTION_SOCIALE',    'chambre', 'Protection sociale',        7),
    ('chambre:PROCEDURES_COLLECTIVES','chambre', 'Procédures collectives',    8),
    ('chambre:INSTRUCTION',           'chambre', 'Chambre de l''instruction', 9),
    ('chambre:FAMILLE',               'chambre', 'Chambre de la famille',     10),
    ('chambre:BAUX',                  'chambre', 'Chambre des baux',          11),
    ('chambre:CONSTRUCTION',          'chambre', 'Chambre de la construction',12),
    ('chambre:ETRANGERS',             'chambre', 'Étrangers et rétention',    13),
    ('chambre:CONSEIL',               'chambre', 'Chambre du conseil',        14),
    ('chambre:EXPROPRIATION',         'chambre', 'Expropriation',             15),
    ('chambre:PROXIMITE',             'chambre', 'Proximité',                 16),
    ('chambre:SURENDETTEMENT',        'chambre', 'Surendettement',            17),
    ('chambre:COPROPRIETE',           'chambre', 'Copropriété',               18),
    ('chambre:URGENCES',              'chambre', 'Urgences',                  19),
    ('chambre:DALO',                  'chambre', 'Droit au logement (DALO)',  20),
    ('chambre:MINEURS',               'chambre', 'Chambre des mineurs',       21),
    ('chambre:NATIONALITE',           'chambre', 'Nationalité',               22),
    ('formation:A_TROIS',          'formation', 'Formation à trois',       1),
    ('formation:A_CINQ',           'formation', 'Formation à cinq',        2),
    ('formation:JUGE_UNIQUE',      'formation', 'Juge unique',             3),
    ('formation:CHAMBRE_SEULE',    'formation', 'Chambre jugeant seule',   4),
    ('formation:RESTREINTE',       'formation', 'Formation restreinte',    5),
    ('formation:SECTION',          'formation', 'Formation de section',    6),
    ('formation:PLENIERE',         'formation', 'Formation plénière',      7),
    ('formation:MIXTE',            'formation', 'Formation mixte',         8),
    ('formation:SSR',              'formation', 'Sous-sections réunies',   9),
    ('formation:CHAMBRES_REUNIES', 'formation', 'Chambres réunies',        10),
    ('formation:ASSEMBLEE',        'formation', 'Assemblée du contentieux',11),
    ('formation:SPECIALISEE',      'formation', 'Formation spécialisée',   12),
    ('office:JUGE_REFERES',                  'office', 'Juge des référés',                        8),
    ('office:PRESIDENT_SECTION_CONTENTIEUX', 'office', 'Président de la section du contentieux',  9),
    ('office:JUGE_EXPROPRIATION',            'office', 'Juge de l''expropriation',                10);
