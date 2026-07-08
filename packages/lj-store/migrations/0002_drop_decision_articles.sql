-- Migration 0002 — Suppression de decision_articles
--
-- La table n'est jamais alimentée : l'extraction d'articles a été retirée
-- du pipeline d'ingestion (cf. ADR 0020). Suppression propre du schéma.

DROP TABLE IF EXISTS decision_articles;
