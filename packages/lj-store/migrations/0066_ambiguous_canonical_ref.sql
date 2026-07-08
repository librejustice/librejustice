-- Migration 0066 — Garde anti-re-merge : liste des `canonical_ref` ambigus
-- (clés constatées NON-uniques), ADR 0103.
--
-- `canonical_ref` (`type|lieu|rg|date`, ADR 0100) n'est pas garanti unique : pour
-- une classe d'arrêts de cour d'appel, le champ judilibre `numbers` porte le RG
-- de la décision DÉFÉRÉE (1re instance), partagé entre plusieurs arrêts d'appel
-- distincts (le vrai RG d'appel ne vit que dans l'en-tête du `full_text`, à
-- couverture partielle — investigation 2026-06-15). Résultat : `resolve_identity`
-- et `fetch_duplicate_key_groups` collent des décisions distinctes (faux merge,
-- ≈3,6 % des clusters multi-provenances mesurés par le juge LLM #43).
--
-- Cette table marque les clés constatées ambiguës (par le juge #43 + revue, #29).
-- `resolve_identity` cesse de rattacher par `canonical_ref` pour ces clés (l'ECLI,
-- unique, reste autoritaire) ; `fetch_duplicate_key_groups` les exclut du
-- regroupement par `canonical_ref`. But : 0 faux merge futur sur les clés connues
-- ambiguës, au prix d'un sous-merge assumé (arbitrage utilisateur 2026-06-15).
-- L'axe `ecli` n'est JAMAIS désactivé (lui est unique).

CREATE TABLE IF NOT EXISTS ambiguous_canonical_ref (
    canonical_ref TEXT PRIMARY KEY,
    n_distinct    INT,
    reason        TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
