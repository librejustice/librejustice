-- Migration 0075 — retrait de l'index trigram title_key (ADR 0112 phase B, P5).
--
-- Le GATE P5 (sonde de couverture vs corpus complet) a tranché : la résolution
-- citation→texte par match EXACT `normalize_instrument(forme) == title_key` +
-- pont curé `TREATY_ALIASES` couvre 95,1 % du volume de citations (vs 88,5 %
-- pour le snap fréquentiel ADR 0079 qu'elle remplace). Le résolveur FUZZY
-- (candidats trigram + best_catalogue_match) n'ajoutait que +0,03 pp — sa seule
-- prise réelle, les coquilles OCR, pèse quelques centaines d'arêtes. Abandonné
-- (pas de code mort, AGENTS.md #11/#12) : on retire l'index qui ne le servait
-- que lui (0074), et l'extension pg_trgm, plus référencée nulle part.
--
-- Réversion additive de 0074 : aucun chemin vif ne lit cet index.

DROP INDEX IF EXISTS idx_legal_text_title_key_trgm;
DROP EXTENSION IF EXISTS pg_trgm;
