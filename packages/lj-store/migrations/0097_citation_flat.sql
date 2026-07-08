-- ADR 0145 — citations à plat : la relation du domaine comme stockage.
--
-- « La décision D cite le texte T à la position S » est stockée telle quelle,
-- cible inline, et rafraîchie en bloc par la passe d'extraction (ingest
-- quotidien pour les nouvelles décisions, passe intégrale hebdomadaire pour le
-- fonds). Le linker est un snapshot du catalogue en mémoire — aucun état de
-- résolution persistant. Tout ce qui est métadonnée d'annotation (statuts
-- gold, clés, antécédents) vit dans les fichiers GT du banc, pas ici.
-- Supprime la fondation trois grains de 0096 : `citation_key`,
-- `citation_occurrence_link`, `citation_gold_anchor`, `resolution_run(_diff)`.
-- Les tables 0096 sont vides à cette date (gold jamais chargé) : drop sec.

DROP TABLE resolution_run_diff;
DROP TABLE resolution_run;
DROP TABLE citation_occurrence_link;
DROP TABLE citation_gold_anchor;
DROP TABLE legal_citation;
DROP TABLE citation_key;

CREATE TABLE legal_citation (
    decision_id     int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start      int4 NOT NULL,  -- codepoints sur full_text, convention 0143 (token)
    char_end        int4 NOT NULL,
    ref_text_uid    text REFERENCES legal_text(text_uid),  -- NULL = non lié
    ref_num_key     text,
    extract_version int2 NOT NULL,  -- < 1000 recognizer, 1000 gold (jamais réécrit)
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL)
);
CREATE INDEX idx_lc_ref ON legal_citation (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;
ALTER TABLE legal_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
