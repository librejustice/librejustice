-- ADR 0181 — decision_party : acteurs par décision, résolus vers les
-- référentiels d'entités (ADR 0179).
--
-- Une ligne par valeur NER émise (coordonnées ontologie 0180 : qualité ×
-- côté). `resolve_key` est calculée en Rust (fold_stable + tête de forme
-- dépouillée — le SQL ne replie jamais). `entity_uid` est une référence
-- DOUCE vers entity.uid : pas de FK, les registres se rechargent par
-- remplacement de namespace (DELETE + COPY) ; le relink la repeuple.

CREATE TABLE decision_party (
    decision_id  bigint NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    ord          int    NOT NULL,
    quality      text   NOT NULL CHECK (quality IN
                     ('party', 'law_firm', 'counsel_name', 'intervenor')),
    side         text   CHECK (side IN ('applicant', 'defendant')),
    value        text   NOT NULL,
    resolve_key  text   NOT NULL,
    entity_uid   text,
    PRIMARY KEY (decision_id, ord)
);

-- Relink : clés pendantes du périmètre résoluble (V1 : morales).
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL AND quality IN ('party', 'intervenor', 'law_firm');
-- Page entité / recherche par acteur.
CREATE INDEX decision_party_entity_idx ON decision_party (entity_uid)
    WHERE entity_uid IS NOT NULL;
