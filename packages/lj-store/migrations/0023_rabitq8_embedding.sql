-- Convertit decision_chunks.embedding de vector(1024) en rabitq8(1024).
--
-- Gains attendus : ~4 GB → ~1 GB (colonne) + ~6.5 GB → ~1.5 GB (index).
-- Recall loss mesuré sur corpus : < 0.0002 d'erreur cosine absolue.
--
-- Coût opérationnel : réécriture complète de la table + reconstruction index.
-- Sur 1 M de chunks, compter ~15–30 min selon I/O disque.

DROP INDEX IF EXISTS chunks_vec;

ALTER TABLE decision_chunks
    ALTER COLUMN embedding TYPE rabitq8(1024)
    USING quantize_to_rabitq8(embedding)::rabitq8(1024);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops);
