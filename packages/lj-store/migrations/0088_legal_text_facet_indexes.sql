-- ADR 0133 — facettes de la recherche d'articles : index B-tree sur les axes filtrables.
-- La recherche `/recherche-textes` filtre et facette sur `legal_text.jurisdiction` et
-- `legal_text.nature` (axe `source` porté par `legal_article`, déjà couvert par ses
-- index). Ces colonnes basse-cardinalité accélèrent le WHERE/GROUP BY du JOIN catalogue.
-- Additif : aucune réécriture de `legal_article`, aucune reconstruction d'index BM25.
CREATE INDEX IF NOT EXISTS legal_text_jurisdiction_idx ON legal_text (jurisdiction);
CREATE INDEX IF NOT EXISTS legal_text_nature_idx ON legal_text (nature);
