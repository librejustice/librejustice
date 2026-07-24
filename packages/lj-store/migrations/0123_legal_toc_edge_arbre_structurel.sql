-- 0123 — legal_toc_edge : arbre structurel daté des textes DILA (ADR 0207).
-- Une ligne par enfant (article ou section) tel que listé par son propriétaire
-- (`texte/struct` → owner = cid du texte ; `section_ta` → owner = id de version
-- LEGISCTA). Écriture par remplacement par propriétaire, lecture par CTE
-- récursive filtrée par fenêtre [date_debut, date_fin) — cf. ADR 0207.

CREATE TABLE legal_toc_edge (
    owner_uid     TEXT NOT NULL,
    text_uid      TEXT NOT NULL,
    seq           INTEGER NOT NULL,
    child_kind    TEXT NOT NULL,
    child_uid     TEXT NOT NULL,
    child_cid     TEXT,
    child_num_key TEXT,
    label         TEXT NOT NULL,
    etat          TEXT NOT NULL,
    date_debut    DATE,
    date_fin      DATE,
    niv           INTEGER,
    PRIMARY KEY (owner_uid, seq)
);

-- Purge par texte (backfill, prune) et amorçage de la CTE côté texte.
CREATE INDEX idx_legal_toc_edge_text ON legal_toc_edge (text_uid);
-- Résolution des ancres : cible section d'un legal_link (LEGISCTA de version)
-- → cid stable ; et vue-lecture par cid.
CREATE INDEX idx_legal_toc_edge_child_uid ON legal_toc_edge (child_uid);
CREATE INDEX idx_legal_toc_edge_child_cid ON legal_toc_edge (child_cid)
    WHERE child_cid IS NOT NULL;
