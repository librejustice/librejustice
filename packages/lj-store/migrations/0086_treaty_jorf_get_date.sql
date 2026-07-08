-- ADR 0129 (corr) — `source_asof` effectif ne doit JAMAIS être inconnu : au pire la
-- « date de get ». jorf/treaty viennent du bulk DILA (ingest manuel/périodique, PAS du
-- sync quotidien comme legifrance/kali) → leur fraîcheur = la **date de get**, stable,
-- stockée par ligne (et non dérivée du live qui avancerait à tort chaque jour).
--
-- Backfill des lignes existantes (date de get inconnue rétroactivement → plancher = date
-- de cette migration : « présence confirmée au plus tard à cette date »). Désormais
-- l'ingest JORF stampe la date de get par ligne (jorf.rs). On retire jorf du live :
-- seuls legifrance/kali sont re-synchronisés quotidiennement (cf. crontab).

UPDATE legal_article SET source_asof = CURRENT_DATE
    WHERE source IN ('treaty', 'jorf') AND source_asof IS NULL;

DELETE FROM ingest_freshness WHERE source = 'jorf';
