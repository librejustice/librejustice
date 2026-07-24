-- 0148 — Purge des fiches TNC ancrées sur un id de version (ADR 0225).
--
-- Depuis que la DILA versionne les textes non codifiés, `META_COMMUN/ID` d'un
-- TEXTE_VERSION LEGI est un id de VERSION (`LEGITEXT…`) distinct du CID
-- chronique (`JORFTEXT…`) sur lequel s'ancrent les articles et la TOC. Le
-- pipeline créait donc une fiche-coquille par version (sans articles ni
-- corps) pendant que les articles vivaient sous le CID, sans fiche. Le parser
-- est ré-ancré sur le CID (ADR 0225) ; on purge ici les coquilles —
-- `backfill-textes` recrée les fiches manquantes keyées CID.

-- La FK `text_legal_citation.ref_text_uid → legal_text` (CASCADE, 0147)
-- n'avait pas d'index côté référent : chaque ligne supprimée de `legal_text`
-- coûtait un seq scan des ~3 M citations (le piège de la règle #2, cf.
-- l'incident 0145). Indispensable AVANT le DELETE de masse ci-dessous.
CREATE INDEX IF NOT EXISTS idx_tlc_ref ON text_legal_citation (ref_text_uid);

-- Liens du graphe dont l'owner est une coquille (arêtes de niveau texte
-- écrites sous l'id de version) : orphelins après la purge, on les retire.
DELETE FROM legal_link ll
USING legal_text lt
WHERE ll.owner_text_uid = lt.text_uid
  AND lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

-- Seule FK sans cascade vers legal_text : les citations liées à une coquille
-- redeviennent non liées (le linker les re-résoudra sur la fiche CID).
UPDATE legal_citation lc SET ref_text_uid = NULL
FROM legal_text lt
WHERE lc.ref_text_uid = lt.text_uid
  AND lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

DELETE FROM legal_text lt
WHERE lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);
