-- ADR 0196 — documents non codifiés : corps monolithique + graphe texte→décision.
--
-- 1. `legal_text.body` : corps d'un texte SANS articles numérotés (circulaires,
--    réponses ministérielles…). Un texte a un corps OU des articles (OU les
--    deux) ; NULL pour les familles à articles (codes, BOFiP, KALI…).
--
-- 2. Index BM25 `legal_text_body_bm25` : jambe corps du scope /recherche-textes
--    pour les textes à body (mêmes tokenizers que `legal_article_bm25`, 0078).
--    Table ~180 k lignes quasi toutes NULL au moment de la pose → build trivial,
--    il grossit avec les ingest de familles à corps.
--
-- 3. `text_case_citation` : la 4ᵉ case du graphe de citations (ADR 0196 §5) —
--    un texte/article cite une décision (§ BOFiP commentant un arrêt, circulaire
--    citant une jurisprudence). Miroir de `case_citation` (0113/0165) côté cible
--    (`target_ref` pendante par famille + `target_decision_id` résolu, relink
--    post-ingest) ; émetteur = article `(owner_text_uid, owner_num_key,
--    owner_date_debut)` ou, si `owner_num_key`/`owner_date_debut` sont NULL, le
--    corps `legal_text.body` du texte. Offsets codepoints sur le texte émetteur
--    (convention 0143 : token identifiant).

ALTER TABLE legal_text ADD COLUMN body text;

-- État de diffusion du texte lui-même (familles sans articles porteurs de
-- statut : circulaires V/A du fond DILA). NULL = non renseigné (familles dont
-- l'état vit sur les articles, comme aujourd'hui).
ALTER TABLE legal_text ADD COLUMN status text;

-- ParadeDB n'autorise qu'UN index bm25 par table ; l'index titre historique
-- (0060, ère referential_texts) n'a plus aucun consommateur `@@@` — le nouvel
-- index couvre title + body.
DROP INDEX referential_texts_title_bm25;

CREATE INDEX legal_text_body_bm25 ON legal_text
USING bm25 (
    id,
    title,
    body,
    (nature::pdb.literal),
    (jurisdiction::pdb.literal)
)
WITH (
  key_field = 'id',
  text_fields = '{
    "title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

CREATE TABLE text_case_citation (
    owner_text_uid     text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    owner_num_key      text,          -- NULL = émis par le corps (legal_text.body)
    owner_date_debut   date,          -- NULL = émis par le corps
    char_start         int4 NOT NULL, -- codepoints sur le texte émetteur, convention 0143
    char_end           int4 NOT NULL,
    target_ref         text NOT NULL, -- clé pendante par famille, format case_citation (0165)
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version    int2 NOT NULL,
    CHECK (char_end > char_start),
    CHECK ((owner_num_key IS NULL) = (owner_date_debut IS NULL))
);
-- Identité d'un span (PK impossible : émetteur nullable) — rejouable sans doublon.
CREATE UNIQUE INDEX text_case_citation_owner_span_key
    ON text_case_citation (owner_text_uid, COALESCE(owner_num_key, ''),
                           COALESCE(owner_date_debut, '0001-01-01'::date), char_start);
-- Relink : résoudre les pendants qui visent une décision nouvellement arrivée.
CREATE INDEX text_case_citation_pending_target_idx
    ON text_case_citation (target_ref) WHERE target_decision_id IS NULL;
-- Descente : quels textes citent cette décision ?
CREATE INDEX text_case_citation_target_decision_idx
    ON text_case_citation (target_decision_id) WHERE target_decision_id IS NOT NULL;
