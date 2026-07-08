-- Migration 0089 — couche override de résolution de citations (ADR 0125 §3).
--
-- ADDITIVE : la résolution automatique (`resolve_citations`, const-arrays alias +
-- remap) NE CHANGE PAS. Cette table porte des CORRECTIONS de résolution curables et
-- versionnées, appliquées APRÈS l'auto par `resolve-citations`. Tant qu'elle est
-- vide, le comportement est strictement préservé.
--
-- Généralise `TREATY_ALIASES`/`KALI_ALIASES`/`EU_PRIMARY_ARTICLE_REMAP` (déjà des
-- corrections de résolution en const compilées, ADR 0112 Addendum #2) vers des
-- données DB. L'override CORRIGE la résolution, il ne RECONNAÎT pas : il agit sur une
-- clé DÉJÀ canonique (`text_key`, même clé que `cited_reference`).
--
--   text_key     : clé canonique ciblée (joignable à cited_reference.text_key).
--   article_key  : NULL = override au niveau texte (sinon couple précis).
--   ref_text_uid : cible forcée (legal_text.text_uid) ; NULL = ABSTENTION
--                  (force non-résolu — sûr par construction, non-résolu ≤ faux-résolu).
--   ref_num_key  : article cible forcé (legal_article.num_key).
--   version      : précédence ; l'actif sur (lower(text_key), article_key) est la
--                  version MAX. 'manuel' = constante très grande (toujours gagnant).
--   reason       : pourquoi (auditabilité).
--   source       : 'human' | 'llm:<model>' | 'farm'.
--
-- SCOPE LÉGER : les colonnes id/offsets de `decision_citation` (22 M lignes) sont
-- reportées à Inc.3 — non touchées ici.

CREATE TABLE citation_resolution_override (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_key     TEXT NOT NULL,
    article_key  TEXT,
    ref_text_uid TEXT,
    ref_num_key  TEXT,
    version      INT  NOT NULL,
    reason       TEXT NOT NULL,
    source       TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Lookup de l'override actif (version max) par couple (clé foldée, article).
CREATE INDEX idx_cro_key ON citation_resolution_override (lower(text_key), article_key);

-- Certification de capture (ADR-plan Inc.2-bis, dormant) : NULLABLE, ne change rien
-- tant que NULL ; le setter + la règle de skip s'activent avec l'oracle d'Inc.2.
ALTER TABLE decisions ADD COLUMN certified_version INT;
