-- ADR 0245 — entités mono-table : dénominations en jsonb sur entity, folded
-- nulle part en heap (index d'expression seulement). lj_fold est la
-- transcription SQL de canon() (lj-core fold_stable + collapse d'espaces) ;
-- sa conformité est vérifiée ci-dessous sur TOUTES les paires
-- (denomination, folded) déjà calculées par le Rust avant de jeter quoi que
-- ce soit. Le translate couvre la table fold_char (accents français, œ,
-- apostrophe typographique) plus l'intégralité des code points White_Space
-- Unicode (la sémantique de split_whitespace — U+0085 NEL attrapé par la
-- campagne de conformité du 2026-07-20, invisible au \s de Postgres).

CREATE FUNCTION lj_fold(text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT btrim(regexp_replace(
    lower(translate($1,
        'ÀÂÄàâäÉÈÊËéèêëÎÏîïÔÖôöÙÛÜùûüÇçŒœ'
        || U&'\2019\0009\000A\000B\000C\000D\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000',
        'aaaaaaeeeeeeeeiiiioooouuuuuuccoo'
        || '''' || '                        ')),
    '\s+', ' ', 'g'))
$fn$;

-- Tableau des folded de tous les noms d'une entité (courant + historiques) —
-- l'expression du GIN de résolution. Appel qualifié `public.lj_fold` :
-- Postgres 18 évalue les corps SQL sous search_path restreint
-- (pg_catalog, pg_temp) pendant CREATE INDEX.
CREATE FUNCTION lj_fold_all(jsonb) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT coalesce(array_agg(public.lj_fold(e->>'d')), '{}')
FROM jsonb_array_elements($1) AS e
$fn$;

-- Garde de conformité (règle #12 : violation d'hypothèse = erreur franche) :
-- lj_fold doit reproduire canon() byte à byte sur tout l'existant.
DO $$
DECLARE bad bigint;
BEGIN
    SELECT count(*) INTO bad FROM entity
    WHERE lj_fold(denomination) IS DISTINCT FROM denomination_folded;
    IF bad > 0 THEN
        RAISE EXCEPTION 'lj_fold diverge de canon() sur % lignes entity', bad;
    END IF;
    SELECT count(*) INTO bad FROM entity_denomination
    WHERE lj_fold(denomination) IS DISTINCT FROM folded;
    IF bad > 0 THEN
        RAISE EXCEPTION 'lj_fold diverge de canon() sur % lignes entity_denomination', bad;
    END IF;
END $$;

-- Dénominations agrégées sur entity : [{d, du, au}, …], dédupliquées
-- (l'historique SIRENE porte des périodes dupliquées — leçon « actis actis »
-- de l'ADR 0243), dates gardées dans les objets (l'ordre du tableau n'est
-- pas porteur).
ALTER TABLE entity ADD COLUMN denominations jsonb;

UPDATE entity e SET denominations = d.arr
FROM (
    SELECT entity_uid,
           jsonb_agg(DISTINCT jsonb_strip_nulls(jsonb_build_object(
               'd', denomination,
               'du', date_debut::text,
               'au', date_fin::text))) AS arr
    FROM entity_denomination
    GROUP BY entity_uid
) d
WHERE d.entity_uid = e.uid;

UPDATE entity SET denominations = jsonb_build_array(jsonb_build_object('d', denomination))
WHERE denominations IS NULL;

ALTER TABLE entity ALTER COLUMN denominations SET NOT NULL;

-- Bascule des index : les trois index portés par denomination_folded se
-- reconstruisent sur l'expression lj_fold(denomination) ; le GIN devient la
-- surface de résolution (tous les noms). Les noms perdent le préfixe
-- entity_denomination_ (la table disparaît).
DROP INDEX entity_annuaire_idx;
DROP INDEX entity_denomination_prefix_idx;
DROP INDEX entity_denomination_contentieux_idx;
ALTER TABLE entity DROP COLUMN denomination_folded;

CREATE INDEX entity_annuaire_idx
    ON entity (category, decision_count DESC, lj_fold(denomination));
CREATE INDEX entity_prefix_idx
    ON entity (lj_fold(denomination) text_pattern_ops);
CREATE INDEX entity_prefix_contentieux_idx
    ON entity (lj_fold(denomination) text_pattern_ops)
    WHERE decision_count > 0;
CREATE INDEX entity_folded_all_idx
    ON entity USING gin (lj_fold_all(denominations));

DROP TABLE entity_denomination;
