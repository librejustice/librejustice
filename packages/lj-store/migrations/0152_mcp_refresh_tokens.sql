-- Migration 0152 : refresh tokens OAuth pour l'accès MCP (rotation, ADR 0235)
-- Les access tokens (mcp_tokens, 0010) durent 30 j ; sans refresh token, la
-- connexion ChatGPT / Claude meurt en silence à expiration et exige un
-- re-consentement manuel. On ajoute des refresh tokens (90 j) rotés à chaque
-- usage (OAuth 2.1, client public). FK CASCADE users(sub) + mcp_clients
-- (client_id) comme 0043/0044 : supprimer un compte ou un client révoque les
-- refresh tokens en attente.

CREATE TABLE mcp_refresh_tokens (
    refresh_token TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    client_id     TEXT NOT NULL REFERENCES mcp_clients (client_id) ON DELETE CASCADE,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_refresh_tokens (expires_at);
