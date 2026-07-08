-- Migration 0073 — cited_reference / decision_citation (ADR 0112, phase B).
--
-- Modèle de citation unifié : une ligne `cited_reference` par couple
-- (text_key, article_key) canonique cité — le vocabulaire searchable (ADR 0112
-- §10 : résolus ET non-résolus) —, et une arête `decision_citation` par décision
-- citant cette référence. Remplace le 3NF legal_codes / legal_articles /
-- decision_legal_references + legal_article_resolution (droppés en 0075, une fois
-- la parité facettes vérifiée contre la GT du banc).
--
--   text_key    : clé normalisée de l'instrument CANONIQUE (snap fréquentiel
--                 ADR 0079 + recognizer) — joignable à legal_text.title_key.
--   article_key : normalize_article(label), NULL si citation sans article.
--   raw_text    : forme d'affichage représentative de l'instrument (facette,
--                 affichage des non-résolus). NOT NULL.
--   raw_article : libellé d'article affiché, NULL si pas d'article.
--   ref_text_uid / ref_num_key : résolution vers l'identité legal_text /
--                 legal_article, NULL si non catalogué — la facette demeure.
--
-- DDL seul. Le remplissage (≈1,83 M refs + 21,5 M arêtes) est un re-run Rust de
-- resolve-refs sur tout le corpus : text_key exige normalize_instrument + canon
-- (hors SQL). Suivi du recompute des facettes (0074). Tables additives : tant
-- que rien ne les lit, elles ne changent aucun comportement live.

CREATE TABLE cited_reference (
    id           BIGSERIAL PRIMARY KEY,
    text_key     TEXT NOT NULL,
    article_key  TEXT,
    raw_text     TEXT NOT NULL,
    raw_article  TEXT,
    ref_text_uid TEXT,
    ref_num_key  TEXT,
    -- NULLS NOT DISTINCT : un couple (text_key, NULL) = une seule ligne (PG15+).
    UNIQUE NULLS NOT DISTINCT (text_key, article_key)
);

-- Facette instrument : regroupement par text_key (dimension searchable).
CREATE INDEX idx_cited_reference_text_key ON cited_reference (text_key);
-- Résolution inverse (décisions citant un legal_text/article donné, /loi/.../citing).
CREATE INDEX idx_cited_reference_resolved
    ON cited_reference (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;

CREATE TABLE decision_citation (
    decision_id        BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    cited_reference_id BIGINT NOT NULL REFERENCES cited_reference(id) ON DELETE CASCADE,
    PRIMARY KEY (decision_id, cited_reference_id)
);

-- Lookup inverse réf→décisions (le sens décision→réfs est couvert par le préfixe PK).
CREATE INDEX idx_decision_citation_ref ON decision_citation (cited_reference_id);
