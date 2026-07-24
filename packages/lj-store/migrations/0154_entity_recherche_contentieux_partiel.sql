-- ADR 0239 (suite) — recherche annuaire sur le registre complet : un
-- préfixe court (« sci  » ≈ 1,9 M de hits) payait ~5 s de top-N par
-- contentieux sur toute la plage. La recherche passe en deux jambes
-- (entity_search) : top contentieux via cet index partiel (≤ ~260 k
-- lignes), complément alphabétique servi par l'ordre de
-- `entity_denomination_prefix_idx` sans tri.

CREATE INDEX entity_denomination_contentieux_idx
    ON entity (denomination_folded text_pattern_ops)
    WHERE decision_count > 0;
