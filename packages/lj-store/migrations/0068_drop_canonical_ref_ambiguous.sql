-- ADR 0104 — l'invariant « ≤1 provenance active par source et par décision »
-- (resolve_identity source-aware sur l'axe canonical_ref) rend la garde par flag
-- ADR 0103 inutile : aucun re-merge same-source par canonical_ref n'est plus
-- possible, quelle que soit l'ambiguïté de la clé. On retire la colonne ajoutée
-- par 0067 (jamais peuplée — flag=0). History append-only : 0066 (table) puis
-- 0067 (colonne, drop table) puis 0068 (drop colonne).
ALTER TABLE decisions
    DROP COLUMN IF EXISTS canonical_ref_ambiguous;
