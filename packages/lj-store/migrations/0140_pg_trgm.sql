-- Extension pg_trgm (ADR 0203) : similarité trigramme pour les suggestions
-- correctives des filtres MCP `legal_instrument` / `legal_article` — une
-- valeur inconnue (faute de frappe comprise) renvoie les slugs les plus
-- proches par `word_similarity` sur `legal_text.title` / `legal_text.slug`.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
