-- Migration 0043 : FK manquante mcp_tokens/mcp_auth_codes → users(sub)
-- Les tables MCP (0010) précèdent la table users (0034), donc user_id n'a
-- jamais été contraint. Conséquence : delete_me() (DELETE FROM users) ne
-- cascade PAS sur les tokens — un bearer MCP émis avant suppression de compte
-- restait un credential vivant jusqu'à expiration (≤ 30 j). On pose la FK
-- ON DELETE CASCADE pour que la suppression révoque tokens et codes en attente.

-- Nettoie d'abord les orphelins éventuels (comptes déjà supprimés).
DELETE FROM mcp_tokens     t WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.sub = t.user_id);
DELETE FROM mcp_auth_codes c WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.sub = c.user_id);

ALTER TABLE mcp_tokens
    ADD CONSTRAINT mcp_tokens_user_fk
    FOREIGN KEY (user_id) REFERENCES users (sub) ON DELETE CASCADE;

ALTER TABLE mcp_auth_codes
    ADD CONSTRAINT mcp_auth_codes_user_fk
    FOREIGN KEY (user_id) REFERENCES users (sub) ON DELETE CASCADE;
