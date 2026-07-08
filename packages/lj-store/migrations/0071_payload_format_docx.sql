-- Migration 0071 — Étend le CHECK payload_format pour accueillir le DOCX (ADR 0110).
--
-- Certaines décisions CNDA sont publiées en Word (.docx) et non en PDF : le lien
-- « Voir la décision » sert alors un OOXML. Born-digital, leur texte est extrait
-- de `word/document.xml` sans OCR (lj-sources::docx, ADR 0110) puis inséré avec
-- `payload_format = 'docx'`. Les deux CHECK (decision_full_text 0059,
-- decision_sources 0064) étaient à ('xml','json','dila-xml','html','pdf') ⇒
-- violation ⇒ ingest de ces décisions en échec. On les aligne sur +docx.

ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf', 'docx'));

ALTER TABLE decision_sources
  DROP CONSTRAINT decision_sources_payload_format_check;

ALTER TABLE decision_sources
  ADD CONSTRAINT decision_sources_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf', 'docx'));
