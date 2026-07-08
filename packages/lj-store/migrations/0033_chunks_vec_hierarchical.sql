-- Rebuild chunks_vec avec une hiérarchie IVF (build.internal.lists).
--
-- Diag : sans ``lists``, vchordrq construit un index plat → scan séquentiel
-- ordonné par distance approximée. EXPLAIN ANALYZE sur ``/search?jur=TCOM``
-- (5% du corpus) : 18 s, 1.27 M buffers, ``Index Searches: 0``, 39 276 lignes
-- scannées pour 200 hits filtrés. Sans hiérarchie, ``prefilter=on`` empire
-- (bench : ×6) car vchord doit alors balayer toute la table avant de pouvoir
-- s'orienter par distance.
--
-- Choix : ``lists = [2000]`` (1 niveau, ~√3,15M). Sous-cluster moyen ≈ 1575
-- vecteurs. ``probes = '20'`` (1%) côté runtime → recall haute, latence basse.
-- Hiérarchie 2 niveaux pas nécessaire à cette échelle (cf. doc VectorChord :
-- LAION 100M utilise [400, 160000], DEEP 1B utilise [800, 640000]).
--
-- Build long : 15-30 min estimés sur 3,15 M vecteurs (1024 dim, rabitq8).
-- Pendant la migration : SHARE lock sur ``decision_chunks`` → writes bloquées,
-- jambe ANN /search down. Stopper api/cron avant ``librejustice migrate``.

DROP INDEX chunks_vec;

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops)
WITH (options = $$
[build.internal]
lists = [2000]
build_threads = 8
spherical_centroids = true
$$);

ANALYZE decision_chunks;
