-- ADR 0129 — fraîcheur structurée de la source (« as-of ») + source secondaire amont.
-- Deux axes distincts, NON confondus avec `translation` (officialité du texte, ADR 0116) :
--   * `source`            (existant) = diffuseur/domaine (legifrance, jafbase, droitcamerounais.info…) ;
--   * `source_authority`  = autorité du diffuseur — PAS une colonne : mapping pur en Rust (lj-core) ;
--   * `source_asof`       = date « as-of » de fraîcheur (dernière base crédible que la copie est à jour) ;
--   * `source_upstream_url` = source secondaire amont (le site qu'un agrégateur comme jafbase pointe).
-- Par article (PAS au niveau texte) : un traité + ses avenants combine des versions de sources
-- et de dates différentes (cf. CorpusVersion).

ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS source_asof         DATE,
    ADD COLUMN IF NOT EXISTS source_upstream_url TEXT;

-- Fraîcheur des sources VIVANTES autoritaires (legifrance/kali/jorf) : ne PAS la stocker
-- par ligne (l'upsert article est content-gated → ne rafraîchirait pas une ligne inchangée,
-- et réécrire ~1,9 M lignes/jour = bloat). Une ligne par source, rafraîchie à chaque ingest
-- (commande `lj-ingest stamp-freshness`). La fraîcheur effective d'un article se dérive :
--   COALESCE(legal_article.source_asof, ingest_freshness[source]).
CREATE TABLE IF NOT EXISTS ingest_freshness (
    source TEXT PRIMARY KEY,
    asof   DATE NOT NULL
);

-- Seed : on a (re)confirmé la fraîcheur des sources live au moment de cette migration.
INSERT INTO ingest_freshness (source, asof) VALUES
    ('legifrance', CURRENT_DATE),
    ('kali',       CURRENT_DATE),
    ('jorf',       CURRENT_DATE)
ON CONFLICT (source) DO NOTHING;

-- Backfill jafbase : la seule date crédible est la mise à jour du site (ADR 0108).
UPDATE legal_article SET source_asof = DATE '2025-10-04'
    WHERE source = 'jafbase' AND source_asof IS NULL;
