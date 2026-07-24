-- ADR 0192 — annuaire des entités avec contentieux.
--
-- Table de stats matérialisée : une ligne par entité de registre ayant AU
-- MOINS une décision liée (`decision_party.entity_uid`), dénormalisée avec
-- tout ce que l'annuaire affiche (catégorie, identité, décompte). Périmètre =
-- « entités avec contentieux », pas un miroir SIRENE (une fiche vide n'a pas
-- d'intérêt produit, et le périmètre réduit — ~255 k lignes — rend
-- recherche/tri triviaux).
--
-- Reconstruite EN BLOC après chaque relink des parties
-- (`refresh_entity_contentieux` : TRUNCATE + INSERT…SELECT depuis
-- `decision_party` JOIN `entity`) : cohérente avec l'état des registres et de
-- la résolution au moment du run. Pas de colonne sur `entity` (le décompte
-- dépend des décisions, pas du registre).
--
-- `category` (dérivée à la reconstruction depuis namespace de l'uid + nature)
-- porte les 4 catégories produit ; `namespace`/`nature`/`denomination`/… sont
-- dénormalisés pour servir le listing et la recherche SANS jointure. Le
-- pliage `denomination_folded` est copié verbatim d'`entity` (fold_stable du
-- chargeur de registres) : la requête plie la recherche du même fold, côté
-- Rust, et matche en préfixe.

CREATE TABLE entity_contentieux (
    entity_uid          text PRIMARY KEY REFERENCES entity(uid) ON DELETE CASCADE,
    -- entreprises | personnes_publiques | associations | avocats
    category            text NOT NULL,
    namespace           text NOT NULL,
    nature              text NOT NULL,
    denomination        text NOT NULL,
    denomination_folded text NOT NULL,
    forme               text,
    active              bool NOT NULL,
    -- slug de barreau (avocats cnb: uniquement, 2e segment de l'uid).
    barreau             text,
    decision_count      bigint NOT NULL
);

-- Listing paginé d'une catégorie, trié contentieux décroissant puis
-- dénomination pliée : index parfaitement ordonné → LIMIT/OFFSET en index scan
-- (pas de tri). Sert aussi le GROUP BY category de `/entities/stats`.
CREATE INDEX entity_contentieux_listing_idx
    ON entity_contentieux (category, decision_count DESC, denomination_folded);

-- Filtre barreau des avocats (index partiel : seuls les cnb: portent un
-- barreau), même ordre de tri.
CREATE INDEX entity_contentieux_barreau_idx
    ON entity_contentieux (barreau, decision_count DESC, denomination_folded)
    WHERE barreau IS NOT NULL;

-- Recherche par préfixe de dénomination pliée (`LIKE 'prefixe%'`) :
-- `text_pattern_ops` rend le prefix-match index-scannable indépendamment de la
-- collation de la base.
CREATE INDEX entity_contentieux_search_idx
    ON entity_contentieux (denomination_folded text_pattern_ops);

ALTER TABLE entity_contentieux SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
