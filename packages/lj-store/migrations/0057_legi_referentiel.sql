-- Migration 0057 — Référentiel LEGI versionné (ADR 0092).
--
-- DDL bon marché uniquement : tables vides, aucun backfill. L'ingestion du fond
-- LEGI (bootstrap `Freemium_legi_global_*.tar.gz` puis incréments quotidiens) se
-- fait HORS-MIGRATION via `lj-ingest legi`, idempotente sur `legiarti` + checksum
-- (#7). Le pont `legal_codes.legitext` est posé/recalculé hors-migration par
-- `bridge_legal_codes_legitext` (recomputable, ADR 0092).
--
-- Deux vérités-source distinctes (#11) : `legal_codes`/`legal_articles` (libellés
-- bruts cités par les juges, ADR 0079) restent inchangés ; `legi_codes`/
-- `legi_articles` portent le droit positif versionné. Le pont code↔code les relie
-- sans les fusionner.

-- ── Catalogue des codes/textes LEGI ────────────────────────────────────────
-- `legitext` = CID stable (LEGITEXT… / JORFTEXT…). `code_court` = slug du titre
-- (jamais le LEGITEXT brut dans l'URL), nullable.
CREATE TABLE legi_codes (
    legitext              TEXT PRIMARY KEY,
    code_court            TEXT,                  -- slug(titre), nullable
    titre                 TEXT NOT NULL,
    nature                TEXT NOT NULL,         -- CODE / LOI / DECRET / ORDONNANCE…
    derniere_modification DATE
);

CREATE INDEX idx_legi_codes_code_court ON legi_codes (code_court)
    WHERE code_court IS NOT NULL;

-- ── Articles versionnés ────────────────────────────────────────────────────
-- Une ligne par version d'article, clé d'idempotence naturelle `legiarti`.
-- `legitext` n'est PAS une FK : un article peut précéder son texte parent dans
-- un incrément (back-fill différé du `titre_text`, pas de fallback silencieux
-- #12). Les sentinelles `2999-01-01`/`2222-02-22` sont normalisées NULL à la
-- frontière de parsing (#12), jamais stockées.
CREATE TABLE legi_articles (
    id               BIGSERIAL PRIMARY KEY,
    legiarti         TEXT NOT NULL UNIQUE,       -- id de version (idempotence #7)
    legitext         TEXT NOT NULL,              -- code parent (pas de FK : back-fill)
    num              TEXT NOT NULL,              -- "1240", "L. 822-1"
    num_key          TEXT NOT NULL,              -- normalize_article(num) (jointure)
    titre_text       TEXT,                       -- fil d'Ariane TM (back-fillable)
    etat             TEXT NOT NULL,              -- VIGUEUR / ABROGE / MODIFIE…
    date_debut       DATE NOT NULL,
    date_fin         DATE,                       -- NULL = pas de fin (sentinelle 2999→NULL)
    texte            TEXT,
    nota             TEXT,
    content_checksum BIGINT NOT NULL             -- xxh3-64 du bloc XML brut (cast bit-à-bit i64)
);

-- Law-at-date : `(legitext, num_key, date_debut)` borne la recherche de version.
CREATE INDEX idx_legi_articles_at_date ON legi_articles (legitext, num_key, date_debut);
-- Version en vigueur (date absente ⇒ etat='VIGUEUR'). Partiel.
CREATE INDEX idx_legi_articles_vigueur ON legi_articles (legitext, num_key)
    WHERE etat = 'VIGUEUR';

-- ── Pont legal_codes ↔ legi_codes (recomputable, niveau code) ──────────────
-- Posé sur les formes canoniques par matching `canon(name) = canon(titre)`
-- (bridge_legal_codes_legitext). Recomputable comme `canonical_id` (ADR 0079).
ALTER TABLE legal_codes ADD COLUMN legitext TEXT REFERENCES legi_codes(legitext);
