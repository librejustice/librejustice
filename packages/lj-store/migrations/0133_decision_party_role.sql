-- ADR 0194 : rôle explicite des avocats (counsel_name), capté en apposition
-- par l'extracteur. NULL = aucun marqueur dans le texte (règle #12).
ALTER TABLE decision_party ADD COLUMN role text
    CHECK (role IN ('substituant', 'substitue', 'postulant', 'plaidant'));
