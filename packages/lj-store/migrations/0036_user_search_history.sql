-- Migration 0036 : historique de recherche utilisateur
-- ADR : docs/adr/0036-user-profile-local-bookmarks-history.md

CREATE TABLE user_search_history (
    id          BIGSERIAL PRIMARY KEY,
    user_sub    TEXT  NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    query       TEXT  NOT NULL,
    filters     JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_user_search_history_user_created
    ON user_search_history (user_sub, created_at DESC);
