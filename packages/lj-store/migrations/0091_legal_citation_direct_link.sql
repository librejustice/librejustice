-- Migration 0091 — lien-span DIRECT par occurrence (ADR 0139, supersede 0134).
--
-- Tue le duo à deux temps « capture globale (cited_reference) → résolution snap
-- (ref_text_uid) ». Le problème STRUCTUREL : cited_reference est GLOBAL (dédupliqué
-- inter-décisions), donc la résolution ne peut PAS s'appuyer sur le contexte de la
-- décision citante — les anaphores (« la loi précitée »), les codes étrangers
-- ambigus (« code de la famille congolais » → quel Congo ?) et les accords
-- bilatéraux non datés sont soit perdus avant capture, soit snappés à l'aveugle.
--
-- Nouveau modèle : UNE LIGNE PAR OCCURRENCE, la cible (`ref_text_uid`) portée
-- INLINE sur l'occurrence. Une occurrence est une entité de 1ʳᵉ classe : elle a son
-- propre span, sa propre résolution dépendante du contexte, sa propre source
-- (recognizer déterministe | oracle LLM | curateur humain) et son antécédent
-- d'anaphore. C'est exactement la prémisse qu'ADR 0134 avait à raison écartée quand
-- la résolution était un batch GLOBAL recalculable — prémisse désormais inversée.
--
--   char_start / char_end : codepoints sur `decisions.full_text` immuable (ADR
--            0125 §2). NULL = span pas encore émis (ancien fonds backfillé sans
--            positions) → souligné non-cliquable, drainé au reextract.
--   surface_text          : capture verbatim figée (le texte cité tel quel).
--   text_key / article_key: clé canonique (compat résolveur + override 0089).
--   ref_text_uid          : cible résolue (legal_text.text_uid). NULL si non lié.
--   ref_num_key           : article cible (legal_article.num_key).
--   status                : 'LINKED' | 'NON_CITABLE' | 'UNRESOLVED'.
--   non_citable_reason    : 'local_act' | 'private_statut' | ... (si NON_CITABLE).
--   antecedent_char_*     : anaphore — span où l'antécédent est défini (même doc).
--   source                : 'recognizer' | 'llm:<model>' | 'human'.
--   confidence            : [0,1] (oracle/curateur).
--   extract_version       : version du producteur (gating re-extract, ADR 0085).
--
-- ADDITIVE : `cited_reference` + `decision_citation` restent INTACTES. La bascule
-- (résolveur + API + facettes lisent `legal_citation`) et le drop des anciennes
-- tables arrivent APRÈS validation du backfill, dans un incrément séparé. Tant que
-- rien ne lit `legal_citation`, le comportement prod est strictement préservé.
--
-- BACKFILL : PAS ici (un INSERT de ~30 M lignes verrouillerait/gonflerait). Il vit
-- comme job online keyset-batché (`lj-ingest backfill-legal-citation`) qui éclate
-- chaque arête `decision_citation` × unnest(char_starts, char_ends) JOIN
-- `cited_reference` en lignes `source='recognizer'` — les liens existants sont
-- CONSERVÉS comme baseline déterministe ; l'oracle LLM (v=1000) les corrige ensuite
-- par préséance de source, il ne repart jamais de zéro.

CREATE TABLE legal_citation (
    id                    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id           BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start            INT,
    char_end              INT,
    surface_text          TEXT NOT NULL,
    text_key              TEXT,
    article_key           TEXT,
    ref_text_uid          TEXT,
    ref_num_key           TEXT,
    status                TEXT NOT NULL,
    non_citable_reason    TEXT,
    antecedent_char_start INT,
    antecedent_char_end   INT,
    source                TEXT NOT NULL,
    confidence            REAL,
    extract_version       SMALLINT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Overlay corps : toutes les citations d'une décision, dans l'ordre du texte.
CREATE INDEX idx_lc_decision ON legal_citation (decision_id, char_start);

-- Reverse (facettes / backlinks) : quelles décisions citent la cible X. Remplace
-- le JOIN via cited_reference — le comptage facette devra passer en
-- COUNT(DISTINCT decision_id) à la bascule (une occurrence répétée ≠ N citations).
CREATE INDEX idx_lc_ref ON legal_citation (ref_text_uid) WHERE ref_text_uid IS NOT NULL;

-- Compat résolveur / override (0089) : lookup par clé canonique foldée.
CREATE INDEX idx_lc_text_key ON legal_citation (lower(text_key));
