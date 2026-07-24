-- ADR 0188 : évidence barreau des avocats (slug officiel CNB en apposition,
-- posée par l'extracteur) et extension de la résolution à counsel_name.
ALTER TABLE decision_party ADD COLUMN barreau text;

DROP INDEX decision_party_pending_idx;
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL
      AND quality IN ('party', 'law_firm', 'counsel_name');
