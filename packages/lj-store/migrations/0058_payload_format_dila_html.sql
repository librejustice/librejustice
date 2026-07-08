-- Migration 0058 — Étend le CHECK payload_format de decision_full_text
-- (posé par 0019) de ('xml','json') à ('xml','json','dila-xml','html') pour
-- accueillir les payloads bulk DILA ('dila-xml') et les sources HTML. La
-- contrainte de decision_sources (0056) porte déjà ce CHECK étendu ; ici on
-- aligne la colonne historique de 0019. Cf. ADR 0093 (et ADR 0094 pour 'html').

ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html'));
