-- Migration 0010 : tables OAuth 2.1 pour l'accès MCP tiers
-- ADR : voir docs/adr/ (décision structurante MCP auth)

CREATE TABLE mcp_clients (
    client_id   TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE mcp_auth_codes (
    code                    TEXT PRIMARY KEY,
    user_id                 TEXT NOT NULL,
    client_id               TEXT NOT NULL REFERENCES mcp_clients (client_id) ON DELETE CASCADE,
    code_challenge          TEXT NOT NULL,
    code_challenge_method   TEXT NOT NULL DEFAULT 'S256',
    redirect_uri            TEXT NOT NULL,
    expires_at              TIMESTAMPTZ NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_auth_codes (expires_at);

CREATE TABLE mcp_tokens (
    access_token TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL,
    client_id    TEXT NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_tokens (expires_at);
