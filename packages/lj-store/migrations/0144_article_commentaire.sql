-- Commentaires de norme (ADR 0212) : table sœur de `decision_sources` pour les
-- articles de loi/traité. Un commentaire s'ancre sur (text_uid, num_key) ;
-- `num_key` NULL = commentaire du texte entier (ex. débats parlementaires d'une
-- loi, propagés aux articles codifiés via `legal_link` au rendu). Même
-- convention jsonb `commentaires[]` que `decision_sources`, non cherchable —
-- enrichissement de la fiche article. `source_uid` unique = clé d'upsert
-- idempotent (#7) ; `source_rank` inutile (jamais autoritaire pour du texte).
-- `IF NOT EXISTS` : la table a été appliquée en prod sous un numéro provisoire
-- (collision de numéro de migration inter-sessions), la migration doit pouvoir
-- se rejouer sans erreur sous son numéro définitif.
CREATE TABLE IF NOT EXISTS article_commentaire (
    id               BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_uid         TEXT NOT NULL,
    num_key          TEXT,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL UNIQUE,
    content_checksum TEXT NOT NULL,
    payload_format   TEXT NOT NULL DEFAULT 'json',
    source_fields    JSONB NOT NULL,
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at       TIMESTAMPTZ
);

-- Lecture par ancre (page article) : (text_uid, num_key). Partiel sur vivant.
CREATE INDEX IF NOT EXISTS article_commentaire_anchor_idx
    ON article_commentaire (text_uid, num_key)
    WHERE deleted_at IS NULL;
