-- Rôles des textes publiés + fusion de doublons (ADR 0246).
-- `role` : rôle primaire dérivé (backfill rejouable `backfill-text-roles`).
-- `alias_of` : manifestation redondante d'un instrument canonique — la
-- recherche l'exclut, la page redirige, l'uid reste résolvable.

ALTER TABLE legal_text
    ADD COLUMN role text NOT NULL DEFAULT 'instrument'
        CONSTRAINT legal_text_role_check CHECK (role IN
            ('instrument', 'modificatif', 'vehicule', 'habilitation', 'individuel')),
    ADD COLUMN alias_of text REFERENCES legal_text (text_uid);

CREATE INDEX legal_text_role_idx ON legal_text (role)
    WHERE role <> 'instrument';
CREATE INDEX legal_text_alias_of_idx ON legal_text (alias_of)
    WHERE alias_of IS NOT NULL;
