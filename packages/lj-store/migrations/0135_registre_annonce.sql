-- ADR 0197 : annonces des registres (BODACC, JOAFE, comptes des associations).
-- Couche *événements* du référentiel d'entités : une ligne par annonce
-- publiée, liée aux entités par uid déterministe (siren:…, rna:…), sans FK
-- (les registres sont rechargés par remplacement de namespace, ADR 0179).

CREATE TABLE registre_annonce (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- bodacc:<parution>:<type>:<numero> | joafe:<identifiant> | casso:<fichier>
    source_uid     TEXT NOT NULL UNIQUE,
    -- Membre bulk d'origine (PCL_BXA20250005.taz…) — unité d'idempotence.
    source_file    TEXT NOT NULL,
    registre       TEXT NOT NULL, -- bodacc | joafe | comptes_asso
    famille        TEXT NOT NULL, -- procol | creation | … | asso_creation | …
    type_avis      TEXT NOT NULL, -- annonce | rectificatif | annulation
    entity_uids    TEXT[] NOT NULL DEFAULT '{}',
    denomination   TEXT,
    tribunal       TEXT,
    date_evenement DATE,          -- jugement / clôture d'exercice / déclaration
    date_parution  DATE NOT NULL,
    parution       TEXT,          -- numéro de parution (NULL pour comptes_asso)
    numero_annonce INTEGER,
    details        JSONB NOT NULL DEFAULT '{}'::jsonb
);

-- Chronologie d'une fiche entité : annonces d'un uid, tri par parution.
CREATE INDEX registre_annonce_entity_uids_idx ON registre_annonce USING gin (entity_uids);
-- Volumétrie / navigation par registre et famille.
CREATE INDEX registre_annonce_famille_idx ON registre_annonce (registre, famille);
-- Remplacement par fichier-source (DELETE ciblé au re-run).
CREATE INDEX registre_annonce_source_file_idx ON registre_annonce (source_file);

-- Suivi d'ingest par membre bulk : un re-run saute les fichiers au checksum
-- inchangé (xxh3-64 du XML brut, règle #7).
CREATE TABLE registre_annonce_file (
    file_name   TEXT PRIMARY KEY,
    checksum    BIGINT NOT NULL,
    annonces    INTEGER NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
