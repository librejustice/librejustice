-- Migration 0040 : source des recherches (web | mcp) + décisions consultées
-- ADR : docs/adr/0053-source-activite-mcp-decisions-consultees.md
-- Étend ADR 0036 (profil local) : on attribue chaque recherche à son canal
-- d'origine (UI web vs endpoint MCP) et on matérialise les décisions ouvertes.

-- Canal d'origine de la recherche. 'web' par défaut : les lignes existantes
-- proviennent toutes de l'UI (le tracking MCP n'existait pas avant).
ALTER TABLE user_search_history
    ADD COLUMN source TEXT NOT NULL DEFAULT 'web';

-- Décisions consultées, modèle dédupliqué : une ligne par (user, décision).
-- Chaque ouverture bump last_viewed_at + view_count ; last_source porte le
-- canal de la dernière consultation (web | mcp) pour différenciation UI.
CREATE TABLE user_decision_views (
    user_sub        TEXT    NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    decision_id     BIGINT  NOT NULL REFERENCES decisions (id) ON DELETE CASCADE,
    view_count      INTEGER NOT NULL DEFAULT 1,
    last_source     TEXT    NOT NULL DEFAULT 'web',
    first_viewed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_viewed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_sub, decision_id)
);

CREATE INDEX idx_user_decision_views_user_viewed
    ON user_decision_views (user_sub, last_viewed_at DESC);
