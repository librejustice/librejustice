-- Migration 0067 — Garde anti-re-merge : flag porté par `decisions`, pas une table.
-- Supersede la partie « table » de l'ADR 0103 (0066).
--
-- 0066 matérialisait les `canonical_ref` ambiguës dans une table dédiée. Inutile :
-- l'ambiguïté est une propriété de la clé, déjà portée par les lignes `decisions`
-- qui l'utilisent. Le guard ne sert que quand un candidat de fusion EXISTE déjà
-- (`find_decision_by_canonical_ref`) — on lit donc le flag du candidat qu'on
-- récupère de toute façon, sans probe ni table séparée. Le flag est dérivable et
-- recalculable en masse : `UPDATE … SET canonical_ref_ambiguous = (clé ∈ critère)`.
--
-- `resolve_identity` : candidat trouvé + flag → ne fusionne pas (ECLI reste
-- autoritaire). `fetch_duplicate_key_groups` : exclut `WHERE NOT
-- canonical_ref_ambiguous` de l'axe `canonical_ref` (jamais l'axe `ecli`, unique).

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS canonical_ref_ambiguous boolean NOT NULL DEFAULT false;

DROP TABLE IF EXISTS ambiguous_canonical_ref;
