-- ADR 0144 — citations « trois grains, un lien par grain ».
--
-- Grain 1 : l'occurrence (`legal_citation`, recréée MINCE) — immuable, ne
-- porte JAMAIS d'état de résolution. Grain 2 : la clé (`citation_key`) —
-- dictionnaire + résolution de masse rejouable par diff. Grain 3 : l'exception
-- contextuelle (`citation_occurrence_link`) — gold + abstentions forcées,
-- préséance max à la lecture.
--
-- L'ancien `legal_citation` (0091/0095) est DROPPÉ : write-only (0 lecteur),
-- invariant surface=substring cassé sur 100 % des lignes recognizer ; le gold
-- v1000 qu'il portait est rechargeable depuis les JSONL de `state_dir`
-- (`load-gold`). Les triggers 0076 meurent : les arrays de facettes sont
-- désormais calculés par l'écrivain dans la même transaction (les fonctions
-- `_sync_*_legal_instruments_for` restent, appelées explicitement).
-- `cited_reference`/`decision_citation` survivent jusqu'à M5 (dual-write).

-- ── Grain 2 : dictionnaire + résolution de masse ────────────────────────────

CREATE TABLE citation_key (
    id             int4 GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_key       text NOT NULL,
    article_key    text,                    -- NULL = mention d'instrument seul
    -- Signaux structurés = fonction pure key_signals(text_key) de lj-extract
    -- (jamais du contexte de capture — le first-writer-wins de raw_text est
    -- le péché originel que ce modèle supprime).
    nature         int2 NOT NULL,           -- KeyNature : 0 Autre, 1 Code, 2 CodeEtranger,
                                            -- 3 Loi, 4 Decret, 5 Arrete, 6 Ordonnance,
                                            -- 7 Deliberation, 8 Constitution, 9 ReglementUe,
                                            -- 10 DirectiveUe, 11 TraiteAccord, 12 Ccn
    jurisdiction   text,                    -- ISO-2 si code étranger (gentilé DANS la clé)
    act_date       date,                    -- « loi du 10 juillet 1991 » → 1991-07-10
    act_num        text,                    -- '91-647', '604/2013', 'IDCC 1517'
    citability     int2 NOT NULL DEFAULT 0, -- 0 citable, 1 local_act, 2 private, 3 fragment
    signal_version int2 NOT NULL,
    -- Résolution de masse — écrite UNIQUEMENT par resolve_citation_keys (M3),
    -- par diff (recompute mémoire → UPDATE des seules clés changées).
    ref_text_uid   text REFERENCES legal_text(text_uid),
    ref_num_key    text,
    link_rule      int2,                    -- provenance de la règle gagnante ;
                                            -- Override + ref NULL = abstention forcée
    n_decisions    int4 NOT NULL DEFAULT 0, -- compteur matérialisé (backlinks 0 ms)
    UNIQUE NULLS NOT DISTINCT (text_key, article_key),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL),
    CHECK (ref_text_uid IS NULL OR link_rule IS NOT NULL)
);
CREATE INDEX idx_ck_lower_text ON citation_key (lower(text_key));
CREATE INDEX idx_ck_ref ON citation_key (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;
CREATE INDEX idx_ck_pending ON citation_key (id)
    WHERE link_rule IS NULL AND citability = 0;

-- ── Grain 1 : occurrences (drop + recréation mince, ~52 B/ligne) ────────────

DROP TRIGGER IF EXISTS dc_sync_arrays_ins ON decision_citation;
DROP TRIGGER IF EXISTS dc_sync_arrays_del ON decision_citation;
DROP FUNCTION IF EXISTS sync_citation_arrays_ins();
DROP FUNCTION IF EXISTS sync_citation_arrays_del();

DROP TABLE legal_citation;

CREATE TABLE legal_citation (
    decision_id     int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start      int4 NOT NULL,  -- codepoints sur full_text, convention 0143 (token)
    char_end        int4 NOT NULL,
    key_id          int4 NOT NULL REFERENCES citation_key(id),
    extract_version int2 NOT NULL,  -- < 1000 recognizer, 1000 gold
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start)
);
CREATE INDEX idx_lc_key ON legal_citation (key_id, decision_id);
ALTER TABLE legal_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

-- ── Grain 3 : exception contextuelle, préséance max (~10 k lignes) ──────────

CREATE TABLE citation_occurrence_link (
    decision_id  int8 NOT NULL,
    char_start   int4 NOT NULL,
    status       int2 NOT NULL,  -- 0 LINKED, 1 UNKNOWN (pas de revendication),
                                 -- 2 ABSTAIN (lier = mislink), 3 NON_CITABLE
    ref_text_uid text REFERENCES legal_text(text_uid),
    ref_num_key  text,
    link_rule    int2 NOT NULL,  -- 0 GoldOracle, 1 ContextResolver (futur)
    antecedent_char_start int4,  -- anaphores : mention d'instrument antérieure
    antecedent_char_end   int4,
    PRIMARY KEY (decision_id, char_start),
    FOREIGN KEY (decision_id, char_start)
      REFERENCES legal_citation (decision_id, char_start) ON DELETE CASCADE,
    CHECK ((status = 0) = (ref_text_uid IS NOT NULL)),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL),
    CHECK ((antecedent_char_start IS NULL) = (antecedent_char_end IS NULL))
);
CREATE INDEX idx_col_ref ON citation_occurrence_link (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;

-- ── Ancrage RGPD du gold ─────────────────────────────────────────────────────
-- Checksum (xxh3-64 bit-cast i64) de decisions.full_text au moment du load :
-- mismatch ultérieur (ré-anonymisation) ⇒ décision gold quarantainée par le
-- banc et re-ancrée, jamais de dérive silencieuse des offsets.

CREATE TABLE citation_gold_anchor (
    decision_id        int8 PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    full_text_checksum int8 NOT NULL,
    loaded_at          timestamptz NOT NULL DEFAULT now()
);

-- ── Journal du resolve (détecteur de régression en régime permanent) ────────

CREATE TABLE resolution_run (
    id          int4 GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    n_keys      int4 NOT NULL DEFAULT 0,  -- clés candidates examinées
    n_gained    int4 NOT NULL DEFAULT 0,
    n_lost      int4 NOT NULL DEFAULT 0,
    n_moved     int4 NOT NULL DEFAULT 0,
    by_rule     jsonb                     -- décomptes par règle du linker
);

-- Diff persisté des seuls lost/moved (les gained se comptent, un premier run
-- en produit des centaines de milliers).
CREATE TABLE resolution_run_diff (
    run_id           int4 NOT NULL REFERENCES resolution_run(id) ON DELETE CASCADE,
    key_id           int4 NOT NULL,
    old_ref_text_uid text,
    new_ref_text_uid text,
    old_ref_num_key  text,
    new_ref_num_key  text,
    change           int2 NOT NULL,  -- 1 lost, 2 moved
    PRIMARY KEY (run_id, key_id)
);
