-- ADR 0112 P5-7 : drop du modèle de citation 3NF legacy + du snap fréquentiel
-- (ADR 0079, retiré). Le modèle unifié `cited_reference`/`decision_citation`
-- (ancrage catalogue `legal_text`/`legal_article`) est la seule source depuis le
-- cutover P5-6 (migration 0076 : arrays facettes/filtres alimentés par les triggers
-- decision_citation ; lj-api lit cited_reference). Le dual-write 3NF a été retiré du
-- code en P5-7 ; ces tables ne sont donc plus écrites ni lues.
--
-- Recon (2026-06-20) : aucun trigger, aucune fonction, aucune vue ne référence ces
-- tables (la 0076 a rebasculé les fonctions _sync_* sur cited_reference et droppé les
-- triggers dlr). Seules subsistent des FK internes au sous-graphe + vers
-- decisions/legal_text → CASCADE les nettoie. Ordre des DROP indifférent (CASCADE).

DROP TABLE IF EXISTS decision_legal_references CASCADE;
DROP TABLE IF EXISTS legal_article_resolution CASCADE;
DROP TABLE IF EXISTS legal_articles CASCADE;
DROP TABLE IF EXISTS legal_codes CASCADE;
