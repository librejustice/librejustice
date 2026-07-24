-- ADR 0179 — référentiel d'entités (registres externes : SIRENE, RNA…).
--
-- `entity` porte l'état courant d'une entité de registre (uid namespacé
-- 'siren:…', 'rna:…') ; `entity_denomination` porte ses dénominations
-- datées (modèle alias de l'ontologie acteurs — la courante y figure
-- aussi, date_fin NULL). Le pliage (`*_folded`) est calculé par le
-- chargeur Rust (fold_stable lj-core) — le SQL ne replie jamais.
-- Chargement par remplacement de namespace (DELETE LIKE 'ns:%' + COPY
-- binaire, une transaction) : idempotent, rejouable mensuellement.

CREATE TABLE entity (
    uid                 text PRIMARY KEY,
    nature              text NOT NULL CHECK (nature IN
                            ('morale_privee', 'morale_publique', 'physique')),
    denomination        text NOT NULL,
    denomination_folded text NOT NULL,
    sigle               text,
    forme               text,
    active              bool NOT NULL,
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX entity_denomination_folded_idx ON entity (denomination_folded);
ALTER TABLE entity SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- Pas de PK : une même dénomination peut couvrir plusieurs périodes
-- (A→B→A) — la ligne courante a date_fin NULL, l'historique des périodes
-- closes est daté.
CREATE TABLE entity_denomination (
    entity_uid   text NOT NULL REFERENCES entity(uid) ON DELETE CASCADE,
    denomination text NOT NULL,
    folded       text NOT NULL,
    date_debut   date,
    date_fin     date
);
CREATE INDEX entity_denomination_uid_idx ON entity_denomination (entity_uid);
CREATE INDEX entity_denomination_resolve_idx ON entity_denomination (folded);
ALTER TABLE entity_denomination SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
