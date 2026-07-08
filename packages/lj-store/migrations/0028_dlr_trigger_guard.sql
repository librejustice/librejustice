-- Migration 0028 — Guard ``IS DISTINCT FROM`` dans le trigger
-- ``dlr_sync_chunks_legal_instruments``.
--
-- Le trigger 0027 recalcule et UPDATE ``decision_chunks.legal_instruments``
-- inconditionnellement à chaque DML sur ``decision_legal_references``. PG
-- crée alors une nouvelle tuple version même quand l'array recalculé est
-- identique → BM25 + vec ré-indexent ces chunks pour rien → bloat.
--
-- Ce cas est dominant lors d'un cleanup au niveau articles (regrouper deux
-- variants typographiques d'un même article-FK sans toucher au code) :
-- l'array ``legal_instruments`` (= les CODES cités, pas les articles)
-- reste strictement identique. Avec le guard, zéro chunk touché.
--
-- Coût : un SELECT-into supplémentaire pour pré-calculer l'array, mais
-- le UPDATE est exécuté seulement quand utile. Net positif.

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
    new_arr TEXT[];
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    SELECT ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name) INTO new_arr
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    JOIN legal_codes    lc ON lc.id = la.code_id
    WHERE dlr.decision_id = target_decision_id;

    UPDATE decision_chunks
    SET legal_instruments = new_arr
    WHERE decision_id = target_decision_id
      AND legal_instruments IS DISTINCT FROM new_arr;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;
