-- Migration 0059 — Étend le CHECK payload_format de decision_full_text
-- (posé par 0019, déjà élargi en 0058 à 'dila-xml'/'html') pour accueillir le
-- premier fond PDF du corpus : les conclusions du rapporteur public ArianeWeb
-- (CRP), PDF born-digital extrait au bord lj-sources. Cf. ADR 0095.

ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf'));
