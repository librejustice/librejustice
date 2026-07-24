-- ADR 0250 — décomptes de décisions citantes par terme cité.
--
-- `cited_term` reprend l'alphabet du GIN `lj_cit_terms` (ADR 0247) :
-- `text_uid` (usage d'un texte) ou `text_uid|num_key` (usage d'un article).
-- Une ligne `legal_citation` = une décision et `lj_cit_terms` dédoublonne par
-- blob : `count(*)` = nombre de décisions citantes.
--
-- Deux consommateurs (lecture par pkey) :
-- - pages « décisions citantes » : choix du plan (GIN vs marche par récence)
--   selon le volume de citantes ;
-- - co-citations « souvent cité avec » : pondération IDF (remplace la liste
--   en dur CO_CITATION_BOILERPLATE).
--
-- Rebuild hebdomadaire par `resync-legal-arrays` (même dérivée de
-- `legal_citation` que les arrays de facettes).
CREATE TABLE citing_decision_counts (
    cited_term     text   PRIMARY KEY,
    decision_count bigint NOT NULL
);

INSERT INTO citing_decision_counts (cited_term, decision_count)
SELECT term, count(*)
FROM legal_citation, unnest(public.lj_cit_terms(spans)) AS term
GROUP BY term;
