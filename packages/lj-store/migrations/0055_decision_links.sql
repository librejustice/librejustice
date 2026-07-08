-- Migration 0055 — Graphe de citations décision↔décision (ADR 0089).
--
-- Table de liens compacte : une arête = (src, kind, cible), la cible étant
-- ``dst_decision_id`` quand résolue (entier vers ``decisions``), sinon
-- ``dst_key`` (clé minimale hors-corpus : ECLI ou id Judilibre). Aucun texte
-- de référence dupliqué : le texte de la cible vit dans sa propre ligne
-- ``decisions`` quand elle est en corpus, sinon ``dst_key`` porte la clé brute.
--
-- ``kind`` SMALLINT : enum ``DecisionLinkKind`` (lj-dtos, source de vérité,
-- règle #3). Codes croissants sans migration de données.
--
-- Identité naturelle de l'arête = son tuple (pas de ``id`` BIGSERIAL) :
-- contrainte UNIQUE NULLS NOT DISTINCT pour que la clé discrimine sur
-- ``dst_decision_id`` OU ``dst_key`` (l'un est NULL). L'upsert
-- (``replace_decision_links``) s'y appuie pour l'idempotence (#7).

CREATE TABLE decision_links (
    src_decision_id BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    dst_decision_id BIGINT   NULL     REFERENCES decisions(id) ON DELETE SET NULL,
    dst_key         TEXT     NULL,   -- ECLI ou id Judilibre de la cible hors-corpus
    kind            SMALLINT NOT NULL,
    -- une arête a toujours une cible : résolue (entier) ou pendante (clé).
    CONSTRAINT decision_links_target_present
        CHECK (dst_decision_id IS NOT NULL OR dst_key IS NOT NULL)
);

-- Identité naturelle de l'arête. NULLS NOT DISTINCT : (src, kind, id, NULL) et
-- (src, kind, NULL, key) sont chacun uniques même avec un NULL dans la clé.
ALTER TABLE decision_links
    ADD CONSTRAINT decision_links_pk
    UNIQUE NULLS NOT DISTINCT (src_decision_id, kind, dst_decision_id, dst_key);

-- Arêtes sortantes d'une décision (page « décisions citées »).
CREATE INDEX idx_decision_links_src ON decision_links (src_decision_id, kind);
-- Arêtes entrantes résolues (page « qui cite cette décision »). Partiel :
-- ne couvre que les arêtes dont la cible est en corpus.
CREATE INDEX idx_decision_links_dst ON decision_links (dst_decision_id, kind)
    WHERE dst_decision_id IS NOT NULL;
-- Résolution différée : retrouver les arêtes pendantes par clé quand une cible
-- est ingérée plus tard. Partiel : ne couvre que les arêtes non résolues.
CREATE INDEX idx_decision_links_dst_key ON decision_links (dst_key)
    WHERE dst_decision_id IS NULL;

-- ``decisions.links_version`` : version du pipeline d'extraction de liens
-- (constante ``LINKS_VERSION``, mécanique d'``extract_version`` ADR 0083). Le
-- backfill ne re-parse que les décisions dont la version diffère
-- (``WHERE links_version IS DISTINCT FROM LINKS_VERSION``) — reprise après
-- interruption incluse. NULL = jamais passée par le backfill de liens.
ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS links_version smallint;
