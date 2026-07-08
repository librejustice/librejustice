-- Migration 0029 — Trigger ``dlr_sync_chunks_legal_instruments`` en
-- STATEMENT-level (au lieu de ROW-level).
--
-- Contexte : ``replace_legal_references`` (cf. ADR 0034) fait, pour chaque
-- décision ré-extraite, un DELETE bulk puis un INSERT bulk sur
-- ``decision_legal_references``. Avec le trigger 0028 (ROW-level), chaque
-- ligne supprimée/insérée déclenche un fire complet : recalcul de l'agrégat
-- + UPDATE de tous les chunks de la décision. Pour une décision avec K refs,
-- ça donne ~2K fires (K DELETE + K INSERT) → ~2K UPDATEs sur
-- ``decision_chunks`` (la plus grosse table). Le guard ``IS DISTINCT FROM``
-- ne filtre pas les états intermédiaires (la reconstruction passe par
-- l'array vide puis re-grandit), donc chaque fire écrit.
--
-- Avec STATEMENT-level, on récupère les ``decision_id`` impactés via
-- ``REFERENCING OLD/NEW TABLE`` (PG ≥ 10) et on propage en **une seule
-- passe** par statement. Pour une décision ré-extraite : 1 fire DELETE +
-- 1 fire INSERT = 2 UPDATEs au total vs ~2K. Gain estimé : 5–10× sur
-- ``reextract-fields`` quand ``legal_references`` est ré-extrait.
--
-- PG n'accepte qu'un event par trigger avec ``REFERENCING`` ; on déclare
-- donc trois triggers qui partagent un helper SQL.

DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments ON decision_legal_references;

-- Helper : recalcule + propage l'agrégat ``legal_instruments`` pour un
-- ensemble de ``decision_id``. Le LEFT JOIN garantit que les décisions
-- vidées (DELETE de toutes leurs refs) reçoivent ``NULL`` au lieu d'être
-- oubliées.
CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT t.id AS decision_id,
               ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                   FILTER (WHERE lc.name IS NOT NULL) AS arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments = agg.arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND c.legal_instruments IS DISTINCT FROM agg.arr;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_ins() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(SELECT DISTINCT decision_id FROM new_rows)
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_del() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(SELECT DISTINCT decision_id FROM old_rows)
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_upd() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(
            SELECT decision_id FROM new_rows
            UNION
            SELECT decision_id FROM old_rows
        )
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments_ins
    AFTER INSERT ON decision_legal_references
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_ins();

CREATE TRIGGER dlr_sync_chunks_legal_instruments_del
    AFTER DELETE ON decision_legal_references
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_del();

CREATE TRIGGER dlr_sync_chunks_legal_instruments_upd
    AFTER UPDATE ON decision_legal_references
    REFERENCING NEW TABLE AS new_rows OLD TABLE AS old_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_upd();

-- L'ancienne fonction ROW-level ``sync_chunks_legal_instruments`` n'a plus
-- d'usage ; on la drop pour ne pas la laisser flotter.
DROP FUNCTION IF EXISTS sync_chunks_legal_instruments();
