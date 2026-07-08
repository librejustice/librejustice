-- Migration 0118 — spécialisations révélées par le gate de titres (ADR 0170
-- étape 4) : chambre des terres (Polynésie / Nouvelle-Calédonie, aussi
-- « tribunal foncier ») et commission d'indemnisation des victimes (CIVI).
-- Seeds miroir de CHAMBRE_SEED (lj-extract/src/formation.rs).

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('chambre:TERRES', 'chambre', 'Chambre des terres',                23),
    ('chambre:CIVI',   'chambre', 'Indemnisation des victimes (CIVI)', 24);
