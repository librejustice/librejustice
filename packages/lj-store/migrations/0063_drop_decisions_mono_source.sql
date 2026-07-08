-- Migration 0063 — DROP des colonnes mono-source de `decisions` (ADR 0098 §2).
--
-- Clôt la frontière de l'ADR 0098 : `decisions` devient le **canonique pur**,
-- tout le per-source vit sur `decision_sources`. On DROP `source_uid`,
-- `content_checksum` et `source_fields` de `decisions` — descendus dans
-- `decision_sources` par le portage (ADR 0098 §7 passe 1). Le DROP emporte la
-- contrainte `UNIQUE`/`NOT NULL` de `source_uid` (0001) et l'index associé.
--
-- PRÉREQUIS — ordre staged (ADR 0098 Conséquences) : le **portage des
-- provenances doit être TERMINÉ** avant ce DROP. Sinon `source_fields` (payload
-- méta, présent seulement sur `decisions` pour les lignes non portées) serait
-- PERDU. Le portage se fait HORS-MIGRATION, batché par keyset (base low-IOPS) :
--
--     lj-ingest dedup-backfill          -- passe 1 = portage, puis identité + fusion
--   ou au minimum :
--     lj-ingest backfill-decision-sources
--
-- Ces commandes ne déclenchent PAS le migrator (seuls `migrate`, les ingests et
-- le démarrage `lj-server` le font) : on peut donc porter AVANT que ce DROP ne
-- s'applique, sans interblocage.
--
-- Cette migration NE FAIT PAS le portage elle-même : un INSERT...SELECT de ~3M
-- lignes jsonb en une seule transaction saturerait le WAL de la base low-IOPS
-- (raison d'être du portage batché hors-migration). À la place, elle pose un
-- GARDE-FOU : si une `decisions` n'a pas sa provenance dans `decision_sources`,
-- ou si un `source_fields` non-NULL n'a pas été porté, elle ÉCHOUE bruyamment
-- (et le déploiement avec) — AUCUNE donnée perdue. Relancer le portage, puis
-- redéployer.

DO $$
DECLARE
    missing_prov   bigint;
    missing_fields bigint;
BEGIN
    SELECT count(*) INTO missing_prov
    FROM decisions d
    WHERE NOT EXISTS (
        SELECT 1 FROM decision_sources s WHERE s.source_uid = d.source_uid
    );

    SELECT count(*) INTO missing_fields
    FROM decisions d
    WHERE d.source_fields IS NOT NULL
      AND NOT EXISTS (
        SELECT 1 FROM decision_sources s
        WHERE s.source_uid = d.source_uid AND s.source_fields IS NOT NULL
      );

    IF missing_prov > 0 OR missing_fields > 0 THEN
        RAISE EXCEPTION
            'ADR 0098 : portage incomplet — % décision(s) sans provenance, % source_fields non porté(s). Lancer `lj-ingest dedup-backfill` (ou `backfill-decision-sources`) AVANT le DROP (§2/§7).',
            missing_prov, missing_fields;
    END IF;
END $$;

ALTER TABLE decisions
    DROP COLUMN IF EXISTS source_uid,
    DROP COLUMN IF EXISTS content_checksum,
    DROP COLUMN IF EXISTS source_fields;
