-- ADR 0145 M5 : drop de l'ancien monde citations. `legal_citation` (0097) est
-- l'unique modèle — occurrences à plat, liées in-pass par le linker ; les
-- corrections de masse vivent dans le code/TSV du linker, plus en base.
-- (`decision_citation` d'abord : FK vers `cited_reference`.)
DROP TABLE IF EXISTS decision_citation;
DROP TABLE IF EXISTS citation_resolution_override;
DROP TABLE IF EXISTS cited_reference;
