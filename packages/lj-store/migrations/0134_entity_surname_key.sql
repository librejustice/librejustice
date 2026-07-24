-- ADR 0195 : clé patronyme composé des avocats CNB (nom-seul multi-token,
-- pliée, tirets normalisés en espaces) — sous-étage de résolution nom-seul
-- unique national. NULL partout ailleurs (SIRENE, RNA, oacc, noms simples).
ALTER TABLE entity ADD COLUMN surname_key text;
CREATE INDEX entity_surname_key_idx ON entity (surname_key)
    WHERE surname_key IS NOT NULL;
