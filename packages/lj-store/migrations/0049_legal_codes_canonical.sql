-- Migration 0049 — Couche canonique du vocabulaire d'instruments (ADR 0079).
--
-- ``legal_codes.canonical_id`` : auto-référence vers la ligne canonique
-- (NULL = la forme EST canonique, jamais self). Posée par la passe de
-- consolidation batch (``lj-ingest consolidate-legal-refs`` : squelette +
-- alias ADR 0077 puis snap fuzzy ancré sur la fréquence, ``lj_core::canon``),
-- recomputable — re-tuner les seuils = recompute du mapping + re-sync chunks,
-- sans ré-ingérer. Les 5,5 M ``decision_legal_references`` continuent de
-- pointer la ligne brute ; seule la dénormalisation chunks résout
-- ``raw → canonical`` au sync. No-op tant que ``canonical_id`` est NULL.

ALTER TABLE legal_codes
    ADD COLUMN IF NOT EXISTS canonical_id BIGINT REFERENCES legal_codes(id);

ALTER TABLE legal_codes
    ADD CONSTRAINT legal_codes_canonical_not_self CHECK (canonical_id <> id);

-- Sert la garde GC (une cible canonique n'est jamais orpheline), le diff de
-- re-sync et la vérification d'acyclicité de la consolidation.
CREATE INDEX IF NOT EXISTS idx_legal_codes_canonical
    ON legal_codes (canonical_id) WHERE canonical_id IS NOT NULL;

-- =====================================================================
-- Le sync chunks lit la forme CANONIQUE (instruments + composite).
-- =====================================================================
--
-- Reprise du helper de 0048 : ``COALESCE(canon.name, lc.name)`` remplace
-- ``lc.name`` dans ``instruments_arr`` et ``composite_arr``. ``la.label``
-- est inchangé — les labels/numéros d'articles ne sont JAMAIS fuzzés
-- (garde ADR 0079 §4).

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name)
                      ORDER BY COALESCE(canon.name, lc.name))
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name) || '|' || la.label
                      ORDER BY COALESCE(canon.name, lc.name) || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        LEFT JOIN legal_codes    canon          ON canon.id = lc.canonical_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;
