-- Résolution d'article PRÉFIXE-AGNOSTIQUE par texte (§7.4 codes territoriaux PF/NC).
--
-- Problème : les codes de loi-du-pays/arrêtés PF/NC désignent leurs articles avec un
-- préfixe d'instrument (« A. » arrêté, « LP. » loi du pays, parfois « D. »), mais les
-- juridictions les citent avec un préfixe INCOHÉRENT (« L. 111-4 », « D. 111-4 »,
-- « 111-4 » nu) pour le MÊME article — le numéro identifie l'article à lui seul, le
-- préfixe est de la provenance, pas un discriminant. Le résolveur (ADR 0112) matche
-- `legal_article.num_key = cited_reference.article_key` en égalité exacte, et
-- `normalize_article` garde les préfixes `[LRDA]` distincts À RAISON (en métropole
-- `L.111` ≠ `R.111` ≠ `D.111`). On ne peut donc PAS stripper globalement.
--
-- Solution : un flag PAR TEXTE. Posé uniquement sur les textes curés où le curateur
-- garantit l'unicité du cœur numérique (aucun n° sous deux préfixes). Le résolveur
-- ajoute alors, après le match exact, un match sur le cœur préfixe-strippé pour ces
-- seuls textes (cf. resolve_citations étape 4b). Défaut false = comportement exact
-- inchangé pour tout le fonds (LEGI/KALI/JORF, codes métropole).
ALTER TABLE legal_text
    ADD COLUMN IF NOT EXISTS num_prefix_agnostic BOOLEAN NOT NULL DEFAULT false;
