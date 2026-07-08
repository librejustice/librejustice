-- ADR 0029 : suppression des tables jamais utilisées en production.
-- corpus_token_df / corpus_stats : pipeline TF-IDF custom abandonné.
-- semantic_vocabulary / decision_semantic_keywords : pipeline keyword sémantique jamais lancé.
-- mcp_clients : registre OAuth 2.1 anticipé, jamais implémenté.

DROP TABLE IF EXISTS decision_semantic_keywords;
DROP TABLE IF EXISTS semantic_vocabulary;
DROP TABLE IF EXISTS corpus_token_df;
DROP TABLE IF EXISTS corpus_stats;
DROP TABLE IF EXISTS mcp_clients CASCADE;
