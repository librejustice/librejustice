-- La résolution des case_citation (0113) sonde le GIN docket_numbers une fois
-- par clé pendante. Avec fastupdate (défaut), chaque UPDATE de decisions
-- (ré-extraction de masse : extract_version + champs) empile ses entrées dans
-- la pending list GIN, que CHAQUE lookup rescanne linéairement (mesuré :
-- 88k tuples / 362 pages → 9 ms par sonde au lieu de <1 ms). L'index est
-- write-heavy en rafale mais lookup-critique : insertion directe.
ALTER INDEX idx_decisions_docket_numbers SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_decisions_docket_numbers');
