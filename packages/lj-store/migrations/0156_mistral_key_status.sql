-- Statut des clés Mistral du pool chat : empreinte xxh3-64, jamais le secret.
CREATE TABLE mistral_key_status (
    fingerprint text PRIMARY KEY,
    disabled_until timestamptz NOT NULL,
    last_status smallint NOT NULL,
    marked_by text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
