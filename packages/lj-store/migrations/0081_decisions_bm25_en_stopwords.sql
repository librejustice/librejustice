-- Migration 0081 — Stopwords anglais (sans collision FR) sur decisions_bm25 (ADR 0120).
--
-- Corpus bilingue FR-prioritaire (ADR 0120, supersede 0094 §FR-only) : on ingère
-- désormais la version FR d'une décision européenne si elle existe, sinon la
-- langue disponible (CEDH EN-only). Le texte anglais atterrit dans
-- ``decisions.full_text``, indexé par ``decisions_bm25`` (seul index BM25 de
-- recherche depuis le DROP de ``chunks_bm25`` en 0054). Sans intervention, ses
-- mots-outils anglais (the/of/and…) ne sont pas filtrés (``stopwords_language``
-- ne prend qu'UNE langue) et polluent l'index. On étend donc la liste custom
-- ``stopwords`` (FR via ``stopwords_language`` + ajouts EN ici).
--
-- Liste EN dérivée d'une stopword-list anglaise standard PUIS filtrée contre un
-- lexique français de 330 k mots APRÈS ascii_folding : tout mot anglais dont la
-- forme repliée existe en français est EXCLU (collision). 26 écartés à ce titre,
-- dont ``but`` (le but), ``or`` (l'or / conj.), ``as`` (un as / tu as), ``on``
-- (pron.), ``ours`` (un ours), ``the`` (replié = ``thé``), ``for`` (le for
-- intérieur), ``he/me/no/are/be/in/if…``. ``i`` isolé exclu aussi (marqueur de
-- subdivision « article I » des textes FR). Restent 103 mots EN strictement
-- non-français. Pas de stemmer (cohérent 0079 : il s'appliquerait avant
-- l'ascii_folding et casserait les requêtes) ; le sémantique multilingue est
-- couvert par les embeddings (Qwen3-Embedding, ``chunks_vec``), pas par BM25.
--
-- Recrée l'index (DROP + CREATE) — config tokenizer immuable côté ParadeDB. Le
-- rebuild (~11 GB, 3 M décisions) prend un lock AccessExclusive sur ``decisions``
-- le temps du CREATE : la jambe BM25 de /search est down (la jambe vectorielle
-- reste servie) ; même classe de fenêtre que le VACUUM FULL hebdo. Définition
-- strictement identique à 0053 hormis le tableau ``stopwords``.

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DROP INDEX IF EXISTS decisions_bm25;

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }'
);

ANALYZE decisions;
