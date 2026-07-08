-- Migration 0037 : adapte mcp_clients à RFC 7591 (Dynamic Client Registration)
-- ADR 0037 : OAuth 2.1 + DCR pour Claude.ai / ChatGPT custom connectors

-- Plusieurs redirect_uris par client (Claude.ai et ChatGPT en déclarent un seul,
-- mais la spec RFC 7591 attend un tableau). On (re)crée la table proprement —
-- en local elle a pu être perdue tandis que mcp_auth_codes / mcp_tokens
-- subsistent.

DROP TABLE IF EXISTS mcp_clients CASCADE;

CREATE TABLE mcp_clients (
    client_id     TEXT PRIMARY KEY,
    name          TEXT,
    redirect_uris TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
