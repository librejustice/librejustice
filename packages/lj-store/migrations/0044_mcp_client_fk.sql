-- Migration 0044 : restaure la FK client_id → mcp_clients sur les tables MCP
-- mcp_auth_codes.client_id avait une FK CASCADE en 0010, perdue quand 0037 a
-- fait DROP TABLE mcp_clients CASCADE pour la recréer (RFC 7591). mcp_tokens
-- n'en a jamais eu. On rétablit la contrainte sur les deux pour la cohérence
-- de schéma : supprimer un client OAuth révoque ses tokens et codes en attente.

-- Nettoie d'abord les orphelins (rows antérieures au DROP de 0037).
DELETE FROM mcp_tokens     t WHERE NOT EXISTS (SELECT 1 FROM mcp_clients k WHERE k.client_id = t.client_id);
DELETE FROM mcp_auth_codes c WHERE NOT EXISTS (SELECT 1 FROM mcp_clients k WHERE k.client_id = c.client_id);

ALTER TABLE mcp_tokens
    ADD CONSTRAINT mcp_tokens_client_fk
    FOREIGN KEY (client_id) REFERENCES mcp_clients (client_id) ON DELETE CASCADE;

ALTER TABLE mcp_auth_codes
    ADD CONSTRAINT mcp_auth_codes_client_fk
    FOREIGN KEY (client_id) REFERENCES mcp_clients (client_id) ON DELETE CASCADE;
