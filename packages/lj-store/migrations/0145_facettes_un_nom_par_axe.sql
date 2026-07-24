-- Migration 0145 — un nom par facette sur toutes les couches (ADR 0213).
--
-- Renomme les namespaces d'uid de `facet_value` sur le nom de l'axe côté
-- contrat (juridiction → jurisdiction_type, domaine → legal_domain,
-- chambre → chamber, voie → procedure, portee → significance), les valeurs
-- stockées correspondantes de `decisions` (~3,5 M lignes) et les colonnes
-- `chambre_uid` → `chamber_uid`, `voie_uid` → `procedure_uid`. Replie les
-- clés des filtres d'historique sur le contrat renommé.
--
-- ⚠️ `decisions_bm25` est déposé avant la réécriture de masse : les colonnes
-- réécrites ne sont pas dans l'index, mais chaque UPDATE non-HOT ré-indexerait
-- la ligne entière (full_text retokenisé par Tantivy) — au volume d'un
-- rebuild, en plus du bloat. Rebuild propre en fin de transaction, même
-- fenêtre AccessExclusive que 0081/0142.

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DROP INDEX decisions_bm25;

-- FKs vers facet_value + CHECKs de préfixe déposés le temps du renommage
-- des uids (FK NO ACTION : l'UPDATE du référentiel casserait la référence).
ALTER TABLE decisions
  DROP CONSTRAINT decisions_chambre_uid_fkey,
  DROP CONSTRAINT decisions_voie_uid_fkey,
  DROP CONSTRAINT decisions_legal_domain_uid_fkey,
  DROP CONSTRAINT decisions_voie_uid_check,
  DROP CONSTRAINT decisions_legal_domain_uid_check;

-- Référentiel : uid + facet + parent_uid renommés ensemble (le CHECK
-- `uid LIKE facet || ':%'` se vérifie ligne à ligne).
UPDATE facet_value SET
  uid = CASE facet
    WHEN 'juridiction' THEN 'jurisdiction_type' || substr(uid, 12)
    WHEN 'domaine' THEN 'legal_domain' || substr(uid, 8)
    WHEN 'chambre' THEN 'chamber' || substr(uid, 8)
    WHEN 'voie' THEN 'procedure' || substr(uid, 5)
    WHEN 'portee' THEN 'significance' || substr(uid, 7)
  END,
  parent_uid = CASE
    WHEN parent_uid LIKE 'domaine:%' THEN 'legal_domain' || substr(parent_uid, 8)
    WHEN parent_uid LIKE 'juridiction:%' THEN 'jurisdiction_type' || substr(parent_uid, 12)
    ELSE parent_uid
  END,
  facet = CASE facet
    WHEN 'juridiction' THEN 'jurisdiction_type'
    WHEN 'domaine' THEN 'legal_domain'
    WHEN 'chambre' THEN 'chamber'
    WHEN 'voie' THEN 'procedure'
    WHEN 'portee' THEN 'significance'
  END
WHERE facet IN ('juridiction', 'domaine', 'chambre', 'voie', 'portee');

-- Valeurs stockées par décision (une passe, ~3,5 M lignes réécrites).
UPDATE decisions SET
  chambre_uid = CASE WHEN chambre_uid IS NULL THEN NULL
    ELSE 'chamber' || substr(chambre_uid, 8) END,
  voie_uid = CASE WHEN voie_uid IS NULL THEN NULL
    ELSE 'procedure' || substr(voie_uid, 5) END,
  legal_domain_uid = CASE WHEN legal_domain_uid IS NULL THEN NULL
    ELSE 'legal_domain' || substr(legal_domain_uid, 8) END
WHERE chambre_uid IS NOT NULL
   OR voie_uid IS NOT NULL
   OR legal_domain_uid IS NOT NULL;

ALTER TABLE decisions RENAME COLUMN chambre_uid TO chamber_uid;
ALTER TABLE decisions RENAME COLUMN voie_uid TO procedure_uid;

ALTER TABLE decisions
  ADD CONSTRAINT decisions_chamber_uid_fkey
    FOREIGN KEY (chamber_uid) REFERENCES facet_value(uid),
  ADD CONSTRAINT decisions_procedure_uid_fkey
    FOREIGN KEY (procedure_uid) REFERENCES facet_value(uid),
  ADD CONSTRAINT decisions_legal_domain_uid_fkey
    FOREIGN KEY (legal_domain_uid) REFERENCES facet_value(uid),
  ADD CONSTRAINT decisions_procedure_uid_check
    CHECK (procedure_uid LIKE 'procedure:%'),
  ADD CONSTRAINT decisions_legal_domain_uid_check
    CHECK (legal_domain_uid LIKE 'legal_domain:%');

-- Filtres d'historique (display-only, chips) : clés repliées sur la
-- projection serde camelCase du SearchRequest courant. Même mécanique que
-- 0143 — axes renommés, strates snake_case résiduelles, et purge des clés
-- d'un contrat disparu (mainOutcome, specialProcedure, jurisdictionName,
-- instanceLevel, jurisdictionLevel, publicationCodes, is_*) que plus aucun
-- champ ne sait afficher.
UPDATE user_search_history
SET filters = (filters - 'chambre') || jsonb_build_object('chamber', filters -> 'chambre')
WHERE filters ? 'chambre';
UPDATE user_search_history
SET filters = (filters - 'voie') || jsonb_build_object('procedure', filters -> 'voie')
WHERE filters ? 'voie';
UPDATE user_search_history
SET filters = (filters - 'portee') || jsonb_build_object('significance', filters -> 'portee')
WHERE filters ? 'portee';
UPDATE user_search_history
SET filters = (filters - 'ai_mode') || jsonb_build_object('aiMode', filters -> 'ai_mode')
WHERE filters ? 'ai_mode';
UPDATE user_search_history
SET filters = (filters - 'date_from') || jsonb_build_object('dateFrom', filters -> 'date_from')
WHERE filters ? 'date_from';
UPDATE user_search_history
SET filters = (filters - 'date_to') || jsonb_build_object('dateTo', filters -> 'date_to')
WHERE filters ? 'date_to';
UPDATE user_search_history
SET filters = (filters - 'legal_article') || jsonb_build_object('legalArticle', filters -> 'legal_article')
WHERE filters ? 'legal_article';
UPDATE user_search_history
SET filters = (filters - 'legal_instrument') || jsonb_build_object('legalInstrument', filters -> 'legal_instrument')
WHERE filters ? 'legal_instrument';
UPDATE user_search_history
SET filters = filters - ARRAY[
  'mainOutcome', 'main_outcome', 'specialProcedure', 'special_procedure',
  'jurisdictionName', 'jurisdiction_name', 'instanceLevel', 'instance_level',
  'jurisdictionLevel', 'jurisdiction_level', 'publicationCodes',
  'is_tables_lebon', 'is_recueil'
]
WHERE filters ?| ARRAY[
  'mainOutcome', 'main_outcome', 'specialProcedure', 'special_procedure',
  'jurisdictionName', 'jurisdiction_name', 'instanceLevel', 'instance_level',
  'jurisdictionLevel', 'jurisdiction_level', 'publicationCodes',
  'is_tables_lebon', 'is_recueil'
];

CREATE INDEX decisions_bm25 ON decisions USING bm25 (id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), date_lecture) WITH (key_field=id, text_fields='{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }');

ANALYZE decisions;
ANALYZE facet_value;
