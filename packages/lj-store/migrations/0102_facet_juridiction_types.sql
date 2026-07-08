-- Migration 0102 — labels des types de juridiction dans le référentiel
-- (ADR 0146, complète le seed 0100).
--
-- Le niveau 1 de l'arbre juridiction est `juridiction_type` ; ses libellés
-- vivaient uniquement en code (triple copie labels.rs). On les porte dans
-- `facet_value` sous le namespace `juridiction:*` — mêmes garanties (uid FK,
-- une source de vérité, servis par l'API depuis le cache référentiel). Les
-- sigles vivent dans `abbr`.

INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('juridiction:CE',              'juridiction', 'Conseil d''État',                        'CE',           1),
    ('juridiction:CAA',             'juridiction', 'Cour administrative d''appel',           'CAA',          2),
    ('juridiction:TA',              'juridiction', 'Tribunal administratif',                 'TA',           3),
    ('juridiction:CC',              'juridiction', 'Cour de cassation',                      'CC',           4),
    ('juridiction:CA',              'juridiction', 'Cour d''appel',                          'CA',           5),
    ('juridiction:TJ',              'juridiction', 'Tribunal judiciaire',                    'TJ',           6),
    ('juridiction:TCOM',            'juridiction', 'Tribunal de commerce',                   'TCOM',         7),
    ('juridiction:CNDA',            'juridiction', 'Cour nationale du droit d''asile',       'CNDA',         8),
    ('juridiction:CONSTIT',         'juridiction', 'Conseil constitutionnel',                'Cons. const.', 9),
    ('juridiction:TC',              'juridiction', 'Tribunal des conflits',                  'TC',           10),
    ('juridiction:CEDH',            'juridiction', 'Cour européenne des droits de l''homme', 'CEDH',         11),
    ('juridiction:CJUE',            'juridiction', 'Cour de justice de l''Union européenne', 'CJUE',         12),
    ('juridiction:CE_ANALYSE',      'juridiction', 'Analyse (Conseil d''État)',              'Analyse CE',   13),
    ('juridiction:CE_CONCLUSIONS',  'juridiction', 'Conclusions du rapporteur public',       'Concl. RP',    14);
