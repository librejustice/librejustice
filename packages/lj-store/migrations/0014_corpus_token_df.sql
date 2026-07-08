-- Migration 0014 — Fréquence documentaire des tokens du corpus.
--
-- corpus_token_df stocke, pour chaque token normalisé, le nombre de chunks
-- dans lesquels ce token apparaît (document frequency). Utilisé pour calculer
-- l'IDF global sans exposer les internals de ParadeDB/Tantivy.
--
-- corpus_stats stocke des scalaires globaux (ex. total_chunks) pour que
-- IDF = log((1 + N) / (1 + df)) soit calculable côté Python sans COUNT(*).
--
-- Bootstrap : `librejustice db corpus-token-df-bootstrap`
-- Ingest hook : df += 1 pour chaque token des nouveaux chunks.

CREATE TABLE corpus_token_df (
    token TEXT    PRIMARY KEY,
    df    INTEGER NOT NULL
);

CREATE TABLE corpus_stats (
    key   TEXT   PRIMARY KEY,
    value BIGINT NOT NULL
);
