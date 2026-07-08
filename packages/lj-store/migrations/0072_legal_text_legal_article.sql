-- Migration 0072 — Référentiel → legal_text / legal_article (ADR 0112, phase A).
--
-- ADR 0112 sépare IDENTITÉ (text_uid) et PROVENANCE (source, portée par version).
-- Renomme referential_texts→legal_text, referential_articles→legal_article ; sort
-- `source` de l'identité (source_uid GLOBALEMENT unique, vérifié 2026-06-18 :
-- 1 392 582 lignes = 1 392 582 source_uid distincts, 0 collision) ; ajoute
-- `position` (lecture « à la suite » façon Légifrance), `source_url`,
-- `date_texte`/`date_publi`.
--
-- PHASE A : la machinerie de citations (legal_codes / legal_articles /
-- legal_article_resolution / decision_legal_references + triggers de facettes
-- chunks) est INCHANGÉE ici. Seuls ses pointeurs vers le référentiel se recâblent
-- sur la nouvelle identité (text_uid) côté Rust. Sa fusion dans
-- cited_reference / decision_citation + le backfill des 21,5 M arêtes = phase B.
--
-- Données (3,1 M articles, mesuré 2026-06-18), traitées dans la migration
-- (le migrator pose statement_timeout=0) :
--   • date_debut NULL (1 326 495, 43 %) → sentinelle '0001-01-01' (borne ouverte
--     basse) : la nouvelle PK (text_uid,num_key,date_debut) interdit le NULL.
--     Neutre pour le law-at-date (la version couvre [−∞, date_fin]).
--   • 10 009 groupes (text_uid,num_key,date_debut) en doublon (artefacts de
--     segmentation / ré-ingests) → on garde une version par identité (texte le
--     plus long, puis id max) et on supprime le reste.

-- ── legal_text (ex-referential_texts) ───────────────────────────────────────
ALTER TABLE referential_texts RENAME TO legal_text;
ALTER TABLE legal_text RENAME COLUMN source_uid TO text_uid;
ALTER TABLE legal_text ADD COLUMN date_texte DATE;  -- date du texte (signature/adoption) : par quoi on cite
ALTER TABLE legal_text ADD COLUMN date_publi DATE;  -- publication au JO (entrée en vigueur, traçabilité)
-- `source` quitte le référentiel : la provenance est dérivée des versions
-- (legal_article.source) ; le clivage traité ↔ JORF se lit sur
-- jurisdiction ∈ ('INTL','UE') / nature, plus sur source='treaty'.
ALTER TABLE legal_text DROP CONSTRAINT referential_texts_source_source_uid_key;
ALTER TABLE legal_text DROP COLUMN source;
ALTER TABLE legal_text ADD CONSTRAINT legal_text_text_uid_key UNIQUE (text_uid);
ALTER INDEX idx_referential_texts_slug RENAME TO idx_legal_text_slug;
ALTER INDEX idx_referential_texts_title_key RENAME TO idx_legal_text_title_key;
-- referential_texts_pkey (id), referential_texts_title_bm25 (key_field=id) et la FK
-- legal_codes_referential_text_id_fkey suivent le renommage ; `id` reste PK
-- jusqu'à la phase B (legal_codes la référence encore).

-- ── legal_article (ex-referential_articles) ─────────────────────────────────
ALTER TABLE referential_articles RENAME TO legal_article;
ALTER TABLE legal_article RENAME COLUMN text_source_uid TO text_uid;
ALTER TABLE legal_article ADD COLUMN position INTEGER;  -- ordre de lecture réel (≠ tri lexical 26<26-1<26-2) ; rempli au (ré)ingest
ALTER TABLE legal_article ADD COLUMN source_url TEXT;   -- URL provider NON dérivable du source_uid (gisti/onu rehosté) ; NULL si template-dérivable
-- `source` (provenance) et `source_uid` (= identifiant natif provider : LEGIARTI/
-- JORFARTI/treaty/… ; le « CID » d'où l'URL Légifrance se dérive, ADR §Principe 3)
-- sont CONSERVÉS, mais HORS identité.

-- date_debut : sentinelle pour les NULL (la PK l'interdit).
UPDATE legal_article SET date_debut = DATE '0001-01-01' WHERE date_debut IS NULL;

-- dédoublonnage par identité (text_uid, num_key, date_debut) : garde le meilleur.
DELETE FROM legal_article a
USING (
    SELECT id, row_number() OVER (
        PARTITION BY text_uid, num_key, date_debut
        ORDER BY length(coalesce(texte, '')) DESC, id DESC
    ) AS rn
    FROM legal_article
) d
WHERE a.id = d.id AND d.rn > 1;

-- nouvelle identité naturelle (text_uid, num_key, date_debut) ; `id` disparaît
-- (aucune FK ne le référence — legal_article_resolution.article_id → legal_articles,
-- pas ce référentiel).
ALTER TABLE legal_article ALTER COLUMN date_debut SET NOT NULL;
ALTER TABLE legal_article DROP CONSTRAINT referential_articles_source_source_uid_key;
ALTER TABLE legal_article DROP CONSTRAINT referential_articles_pkey;
ALTER TABLE legal_article DROP COLUMN id;
ALTER TABLE legal_article ADD PRIMARY KEY (text_uid, num_key, date_debut);

-- index : la PK (préfixe text_uid,num_key) couvre le law-at-date → l'ancien
-- at_date devient redondant ; vigueur recentré sur l'identité (source hors clé) ;
-- + lecture « à la suite » par position.
DROP INDEX idx_ref_articles_at_date;
DROP INDEX idx_ref_articles_vigueur;
CREATE INDEX idx_legal_article_vigueur ON legal_article (text_uid, num_key) WHERE status = 'VIGUEUR';
CREATE INDEX idx_legal_article_reading ON legal_article (text_uid, position);

-- ── legal_article_resolution : cible l'identité (text_uid), `source` hors clé ──
-- Cache OLD de résolution citation→article (toujours utilisé en phase A par
-- /loi/{code}/{num}/citing) ; fusionné dans cited_reference en phase B. On retire
-- `ref_source` de la clé pour aligner sur l'identité text_uid.
ALTER TABLE legal_article_resolution RENAME COLUMN ref_text_source_uid TO ref_text_uid;
ALTER TABLE legal_article_resolution DROP COLUMN ref_source;
-- DROP COLUMN ref_source supprime déjà idx_lar_reverse (sa clé incluait
-- ref_source) → on recrée seulement, sur la nouvelle clé alignée text_uid.
CREATE INDEX idx_lar_reverse ON legal_article_resolution (ref_text_uid, num_key);
