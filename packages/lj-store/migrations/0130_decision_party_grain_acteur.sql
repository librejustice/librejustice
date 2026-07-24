-- ADR 0182 : decision_party au grain acteur — spans-évidences, nature,
-- version d'extraction. Le fonds backfillé d'avant-vague reste à
-- extract_version 0, nature/spans NULL (réécrit par le reextract).
ALTER TABLE decision_party
    ADD COLUMN char_starts int4[],
    ADD COLUMN char_ends int4[],
    ADD COLUMN nature text CHECK (nature IN
        ('physique', 'morale_privee', 'morale_publique')),
    ADD COLUMN extract_version int2 NOT NULL DEFAULT 0;
ALTER TABLE decision_party ALTER COLUMN extract_version DROP DEFAULT;

-- Gate intervenors (ADR 0182 §7) : la qualité ne s'émet ni ne se résout en
-- prod tant que la campagne moteur n'atteint pas P >= 85 %.
DROP INDEX decision_party_pending_idx;
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL AND quality IN ('party', 'law_firm');
