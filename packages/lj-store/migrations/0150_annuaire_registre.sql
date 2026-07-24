-- ADR 0233 — double décompte de l'annuaire : total du registre par catégorie.
--
-- `entity_contentieux` (0132) ne compte que les entités avec ≥ 1 décision
-- liée ; affiché seul, ce chiffre fait passer les registres pour minuscules
-- (associations : ~1,9 k affichées pour ~2,29 M de RNA chargés). L'annuaire
-- présente désormais les deux nombres ; le total du registre par catégorie
-- est matérialisé ici — le compter à la volée serait un scan de ~15 M lignes
-- d'`entity` (la dérivation entreprises / personnes publiques lit `nature`,
-- hors index).
--
-- Rafraîchie avec `entity_contentieux`, dans la même transaction
-- (`refresh_entity_contentieux`), avec la MÊME dérivation de catégorie
-- (namespace de l'uid × nature). Seedée à la création : l'annuaire affiche
-- les totaux sans attendre le prochain relink.

CREATE TABLE annuaire_registre (
    -- entreprises | personnes_publiques | associations | avocats
    category text PRIMARY KEY,
    total    bigint NOT NULL
);

INSERT INTO annuaire_registre (category, total)
SELECT CASE
         WHEN uid LIKE 'siren:%' AND nature = 'morale_privee'
           THEN 'entreprises'
         WHEN uid LIKE 'siren:%' AND nature = 'morale_publique'
           THEN 'personnes_publiques'
         WHEN uid LIKE 'rna:%' THEN 'associations'
         WHEN uid LIKE 'cnb:%' OR uid LIKE 'oacc:%' THEN 'avocats'
       END AS category,
       count(*)
FROM entity
WHERE uid LIKE 'siren:%' OR uid LIKE 'rna:%'
   OR uid LIKE 'cnb:%' OR uid LIKE 'oacc:%'
GROUP BY 1;
