-- Citations de jurisprudence (ADR 0165) : « la décision D cite la décision C
-- à la position S », jumelle de `legal_citation` pour le span (offsets
-- codepoints, convention 0143 : token identifiant) et de `decision_links`
-- pour la cible. `target_ref` = clé pendante par famille (`cc|1823954`,
-- `constit|2020-800`, `cjue|c-561/19`, `ce|412412`, `cedh|30010/10`,
-- `rg|{jurisdiction_code}|21/04532[|AAAA-MM-JJ]`), sans date obligatoire ;
-- `target_decision_id` posé par la résolution SQL par famille (à l'écriture
-- puis relink post-ingest — une cible peut arriver après ses citations).

CREATE TABLE case_citation (
    decision_id        int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start         int4 NOT NULL,  -- codepoints sur full_text, convention 0143 (token)
    char_end           int4 NOT NULL,
    target_ref         text NOT NULL,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version    int2 NOT NULL,  -- < 1000 recognizer, 1000 gold (jamais réécrit)
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start)
);
-- Relink : résoudre les pendants qui visent une décision nouvellement arrivée.
CREATE INDEX case_citation_pending_target_idx
    ON case_citation (target_ref) WHERE target_decision_id IS NULL;
-- Descente : qui cite cette décision ?
CREATE INDEX case_citation_target_decision_idx
    ON case_citation (target_decision_id) WHERE target_decision_id IS NOT NULL;
ALTER TABLE case_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- Résolution de masse des clés pendantes contre les numéros portés par les
-- décisions cibles (`docket_numbers @> ARRAY[...]`, clé reformatée côté
-- requête au format stocké par famille : CC « 18-23.954 », CE « 412412 »,
-- CJUE « C-561/19 », RG « 21/04532 »). Le GIN de 0009 avait été droppé en
-- 0030 faute de consommateur — la résolution 0165 en est un.
CREATE INDEX idx_decisions_docket_numbers
    ON decisions USING GIN (docket_numbers) WHERE deleted_at IS NULL;
