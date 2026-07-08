-- Migration 0090 — positions de citation : tableaux de spans sur l'arête (ADR 0134).
--
-- Amende ADR 0125 §1 : les positions des mentions d'une citation dans le corps
-- d'une décision vivent comme DEUX TABLEAUX PARALLÈLES sur l'arête existante
-- `decision_citation`, et NON comme « une ligne par occurrence ». La PK
-- (decision_id, cited_reference_id) reste INCHANGÉE — une arête = une ligne, les
-- N mentions vivent dans les tableaux.
--
--   char_starts[i] / char_ends[i] : i-ᵉ mention, en CODEPOINTS sur
--                 `decisions.full_text` immuable (ADR 0125 §2). Invariant
--                 len(char_starts) = len(char_ends).
--   NULL = positions non encore émises (ancien fonds) → souligné non-cliquable,
--          drainé au reextract. `replace_citations` re-pose les deux tableaux.
--
-- Migration ADDITIVE : `ADD COLUMN … INT[]` est metadata-only en Postgres →
-- zéro rewrite, zéro downtime. Le trigger de facettes (0076) compte 1 ligne par
-- (décision, référence) → aucun gonflement, aucun patch trigger requis.

ALTER TABLE decision_citation ADD COLUMN char_starts INT[];
ALTER TABLE decision_citation ADD COLUMN char_ends   INT[];
