-- ADR 0116 — multilingue : original (langue d'origine) + provenance de traduction.
-- Colonnes compagnes sur la ligne FR (PK inchangée) : l'original est rare, souvent
-- NULL, jamais cherché (BM25 reste FR) — une couche front/vérification.

ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS texte_original TEXT,
    ADD COLUMN IF NOT EXISTS lang_original  TEXT,
    ADD COLUMN IF NOT EXISTS translation    TEXT NOT NULL DEFAULT 'non_officiel'
        CHECK (translation IN ('officiel', 'non_officiel', 'automatique'));

-- Backfill de la provenance depuis l'existant (remplace la fiabilité en texte libre
-- de `nota`, ADR 0108 §4) : droit FR / UE / traités = officiel ; jafbase selon nota.
UPDATE legal_article SET translation = 'officiel'
    WHERE source <> 'jafbase';
UPDATE legal_article SET translation = 'officiel'
    WHERE source = 'jafbase' AND nota ILIKE '%texte officiel%';
