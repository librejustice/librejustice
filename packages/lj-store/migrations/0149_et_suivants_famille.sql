-- 0149 — « et suivants » (ADR 0226) : le span de citation porte la locution,
-- la ligne porte le signal `suivants`, et la famille d'articles désignée
-- (frères TOC de l'ancre, section unique, VIGUEUR, cap 20) alimente les
-- arrays de facettes et le menu du span.
--
-- Deux grains, UNE sémantique (leçon ADR 0147 — jamais deux implémentations) :
-- `_suivants_family_keys` porte l'expansion en clés PUBLIQUES (ADR 0209,
-- la forme de `legal_toc_edge.child_num_key`) — consommée par le menu UI ;
-- `_suivants_family` l'enveloppe pour les facettes en forme CITABLE (celle
-- de `legal_citation.ref_num_key`, ex. « L. 3213-2 »). Les conversions
-- citable ↔ publique sont bornées au sous-ensemble trivialement inversible
-- (préfixe simple L/R/D/A + chiffres/tirets, ou numérique nu) ; hors
-- sous-ensemble (suffixes bis/ter, étoiles, préfixes LO/LP) → pas
-- d'expansion côté facettes, conservateur par construction.

ALTER TABLE legal_citation ADD COLUMN suivants boolean NOT NULL DEFAULT false;

-- Ancrage TOC d'un numéro d'article : la recherche (text_uid, child_num_key)
-- n'avait que l'index par texte (scan des dizaines de milliers d'arêtes d'un
-- gros code) ; sert l'expansion et le menu UI.
CREATE INDEX idx_legal_toc_edge_art_num
    ON legal_toc_edge (text_uid, child_num_key) WHERE child_kind = 'article';

-- Famille « N et suivants » en clés publiques : l'ancre et ses frères `seq`
-- supérieurs dans SA section TOC, état VIGUEUR. NULL (pas d'expansion) si :
-- ancre absente de la TOC, ancre sous plusieurs sections aux familles
-- DIVERGENTES (ambiguë — une section LEGI ré-écrite par réforme donne deux
-- owners à famille identique, cas légitime), ou famille hors [2, 20] (texte
-- plat : « article 1 et suivants de la loi X » cite le texte entier). Finie
-- par construction : requête plate par section, aucune récursion.
CREATE FUNCTION _suivants_family_keys(p_uid text, p_num_key text)
RETURNS text[]
LANGUAGE sql STABLE
AS $$
    WITH anchor AS (
        SELECT owner_uid, min(seq) AS seq
        FROM legal_toc_edge
        WHERE text_uid = p_uid AND child_kind = 'article'
          AND child_num_key = p_num_key AND etat = 'VIGUEUR'
        GROUP BY owner_uid
    ),
    members AS (
        SELECT a.owner_uid, e.child_num_key, min(e.seq) AS seq_min
        FROM anchor a
        JOIN legal_toc_edge e
          ON e.owner_uid = a.owner_uid
         AND e.child_kind = 'article' AND e.etat = 'VIGUEUR'
         AND e.seq >= a.seq
        GROUP BY a.owner_uid, e.child_num_key
    ),
    -- Ordre de LECTURE (seq TOC), pas lexicographique (« l3213-10 » suivrait
    -- « l3213-1 » et précéderait « l3213-2 » sinon).
    fams AS (
        SELECT owner_uid, array_agg(child_num_key ORDER BY seq_min) AS fam
        FROM members
        GROUP BY owner_uid
    )
    SELECT CASE WHEN count(DISTINCT fam) = 1
                 AND cardinality(min(fam)) BETWEEN 2 AND 20
                THEN min(fam)
           END
    FROM fams;
$$;

-- Famille en forme citable (grain `ref_num_key`) : ancre citée « L. 3213-1 »
-- → clé publique « l3213-1 » → famille publique → membres re-projetés en
-- citable (« L. 3213-2 ») pour rejoindre le vocabulaire des tokens de
-- facette (options = lignes `legal_citation`, hydrate.rs). Membres non
-- inversibles écartés.
CREATE FUNCTION _suivants_family(p_uid text, p_cited text)
RETURNS text[]
LANGUAGE sql STABLE
AS $$
    WITH pub AS (
        SELECT CASE
            WHEN p_cited ~ '^[LRDA]\. ?\d[0-9-]*$'
                THEN lower(regexp_replace(p_cited, '\. ?', ''))
            WHEN p_cited ~ '^\d[0-9-]*$' THEN p_cited
        END AS anchor_key
    ),
    fam AS (
        SELECT unnest(_suivants_family_keys(p_uid, pub.anchor_key)) AS k
        FROM pub WHERE pub.anchor_key IS NOT NULL
    ),
    citable AS (
        SELECT CASE
            WHEN k ~ '^[lrda]\d[0-9-]*$'
                THEN upper(left(k, 1)) || '. ' || substr(k, 2)
            WHEN k ~ '^\d[0-9-]*$' THEN k
        END AS c
        FROM fam
    )
    SELECT CASE WHEN count(c) >= 2 THEN array_agg(c) END
    FROM citable WHERE c IS NOT NULL;
$$;

-- Resync (détecteur de dérive, resync_legal_arrays_range) : même expansion
-- que l'écrivain, même tri bytewise (0109), même `IS DISTINCT FROM`.
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decisions d
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE d.id = agg.decision_id
          AND (
                d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decision_chunks c
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE c.decision_id = agg.decision_id
          AND (
                c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;
