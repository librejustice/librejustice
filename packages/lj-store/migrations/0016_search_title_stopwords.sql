-- Recrée l'index BM25 sur decisions.search_title avec le filtre stopwords français.
-- L'ancien index (regex sans stopwords) laissait passer "du", "de", "d'", "la"…
-- ce qui faisait remonter des décisions hors-sujet quand la requête contenait
-- des connecteurs (ex. "article L 423-13 du code de l'entrée et du séjour…").
--
-- pdb.simple('stopwords_language=French', 'ascii_folding=true') :
--   - retire les mots-outils français à l'indexation ET à la query (cohérence garantie)
--   - conserve la normalisation ascii (Cergy = cergy, Île-de-France = ile-de-france)

DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, (search_title::pdb.simple('stopwords_language=French', 'ascii_folding=true')))
WITH (key_field = 'id');
