-- Migration 0064 — Étend le CHECK payload_format de decision_sources pour
-- accueillir le PDF born-digital (ADR 0095/0096).
--
-- Le CHECK de decision_full_text.payload_format a déjà été élargi à 'pdf' par
-- 0059 (CRP ArianeWeb), mais le CHECK INLINE de decision_sources (posé par 0056,
-- auto-nommé `decision_sources_payload_format_check`) est resté à
-- ('xml','json','dila-xml','html'). Depuis 0063, `decision_sources` est la table
-- canonique per-source (les colonnes mono-source de `decisions` ont été droppées)
-- et l'upsert de provenance (`upsert_decision_source`) y écrit `payload_format`.
-- La CNDA insère ses décisions avec PDF en `payload_format = 'pdf'` ⇒ violation
-- du CHECK ⇒ ingest CNDA en échec. On aligne donc ce CHECK sur celui de
-- decision_full_text (0059).

ALTER TABLE decision_sources
  DROP CONSTRAINT decision_sources_payload_format_check;

ALTER TABLE decision_sources
  ADD CONSTRAINT decision_sources_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf'));
