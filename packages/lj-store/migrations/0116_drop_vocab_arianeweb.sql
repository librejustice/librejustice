-- Migration 0116 — retrait du vocab juridiction ArianeWeb (ADR 0171).
--
-- `juridiction:CE_ANALYSE` et `juridiction:CE_CONCLUSIONS` (semés par 0102)
-- sont du vocab mort : la source n'a jamais été ingérée (0 décision, 0
-- référence). Quand les analyses/conclusions du CE reviendront, elles ne
-- seront pas des types de juridiction (ADR 0171).

DELETE FROM facet_value
WHERE uid IN ('juridiction:CE_ANALYSE', 'juridiction:CE_CONCLUSIONS');
