-- Migration 0035 : signets utilisateur sur les décisions
-- ADR : docs/adr/0036-user-profile-local-bookmarks-history.md

CREATE TABLE user_bookmarks (
    user_sub     TEXT   NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    decision_id  BIGINT NOT NULL REFERENCES decisions (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_sub, decision_id)
);

CREATE INDEX idx_user_bookmarks_user_created
    ON user_bookmarks (user_sub, created_at DESC);
