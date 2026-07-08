-- Migration 0039 — Colonne ``summary`` pré-calculée sur ``decisions``.
--
-- Contexte : SEO contenu (cf. working-notes/seo-summary-implementation-
-- plan-2026-05-28.md). On pré-calcule un résumé neutre par décision
-- (mistral-small-2506, ≤500 c) qui sert :
--   - ``<meta name="description">`` et ``og:description`` (SSR /page),
--   - card de résultat de recherche,
--   - affichage par défaut sur la page décision en remplacement de
--     l'analyse contextuelle ``/analyse`` (supprimée).
--
-- ``summary_prompt_version`` permet de relancer sélectivement le batch
-- offline si on change le prompt : ``WHERE summary_prompt_version IS
-- NULL OR summary_prompt_version < N``. Pas d'index — l'usage est
-- exclusivement batch offline (seq scan ~2 s sur 3M rows, acceptable).
-- Pas de ``generated_at`` : la version du prompt capture déjà la
-- traçabilité utile.

ALTER TABLE decisions
    ADD COLUMN summary TEXT,
    ADD COLUMN summary_prompt_version SMALLINT;
