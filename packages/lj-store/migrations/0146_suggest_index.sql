-- Migration 0146 — suggest_index : FST d'autocomplétion (ADR 0216).
--
-- Un blob par clé (`ngrams` : le FST partagé jurisprudence/textes), conforme
-- ADR 0205 : le FST est lu d'un bloc au chargement du serveur, jamais requêté
-- ligne à ligne. Reconstruit par `lj-ingest build-suggest`.

CREATE TABLE suggest_index (
    key      text        PRIMARY KEY,
    built_at timestamptz NOT NULL DEFAULT now(),
    fst      bytea       NOT NULL
);
