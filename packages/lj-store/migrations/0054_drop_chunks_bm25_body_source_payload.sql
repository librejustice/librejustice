-- Migration 0054 — Drops post-cutover grain décision (ADR 0084 étape 4 + 0085).
--
-- Récupère l'espace des trois supports rendus redondants par la refonte « texte
-- au grain décision » (ADR 0084) et « payload reconstructible » (ADR 0085) :
--
--   1. ``chunks_bm25`` (~11 GB) — la recherche BM25 passe par ``decisions_bm25``
--      (0053, grain décision) ; plus aucune jambe n'interroge l'index chunk.
--   2. ``decision_chunks.body`` (~20 GB après repack) — le texte vit dans
--      ``decisions.full_text`` ; le chunk redevient (embedding + offsets).
--      Snippets, render, mining, garble et re-extract lisent ``full_text``.
--   3. ``decision_full_text.source_payload_gzip`` (~12 GB) — le payload source
--      brut est reconstructible depuis ``(full_text, source_fields)``
--      (``reconstruct_{json,xml}_payload``) ; parité d'extraction prouvée
--      (banc ``reextract-parity`` : 0 divergence / 2993, dont 1965 JSON).
--      ``payload_format`` est conservé (choix du reconstructeur côté re-extract).
--
-- ⚠️ PRÉREQUIS (sinon casse la prod) : déployer AVANT cette migration le code
-- qui (a) ne lit plus ``body``/``chunks_bm25`` (cutover recherche/render, jambes
-- sur ``decisions_bm25``, lecteurs offline sur ``full_text``), (b) n'écrit plus
-- ``body`` ni ``source_payload_gzip`` à l'ingest, et (c) reconstruit le download
-- XML à la volée. La séquence est : cutover code → deploy → CETTE migration →
-- repack/VACUUM FULL pour rendre l'espace au système de fichiers.
--
-- DROP INDEX non-CONCURRENT + ALTER DROP COLUMN : transaction-safe (la
-- migration tourne en transaction unique). L'espace de l'index est rendu
-- immédiatement ; celui des colonnes l'est au repack/VACUUM FULL ultérieur.

DROP INDEX IF EXISTS chunks_bm25;

ALTER TABLE decision_chunks DROP COLUMN IF EXISTS body;

ALTER TABLE decision_full_text DROP COLUMN IF EXISTS source_payload_gzip;
