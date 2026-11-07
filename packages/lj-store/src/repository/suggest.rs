//! Blob FST d'autocomplétion (`suggest_index`) : écrit par `lj-ingest
//! build-suggest`, lu d'un bloc par `lj-server` au premier `/suggest`.

use crate::error::Result;

use super::DecisionRepository;

/// Clé du blob FST partagé jurisprudence/textes dans `suggest_index` —
/// écrite par `lj-ingest build-suggest`, lue par `lj-api /suggest`.
pub const SUGGEST_FST_KEY: &str = "ngrams";

impl DecisionRepository<'_> {
    /// Dépose (ou remplace) le FST sérialisé sous `key`, `built_at` rafraîchi.
    #[tracing::instrument(name = "db.upsert_suggest_fst", skip(self, fst), fields(db.system = "postgresql", bytes = fst.len()))]
    pub async fn upsert_suggest_fst(&self, key: &str, fst: &[u8]) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO suggest_index (key, fst) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET fst = EXCLUDED.fst, built_at = now()",
                &[&key, &fst],
            )
            .await?;
        Ok(())
    }

    /// Lit le FST sérialisé sous `key`. `None` si aucun build n'a encore tourné.
    #[tracing::instrument(name = "db.fetch_suggest_fst", skip(self), fields(db.system = "postgresql"))]
    pub async fn fetch_suggest_fst(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let row = self
            .conn
            .query_opt("SELECT fst FROM suggest_index WHERE key = $1", &[&key])
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Lot keyset de corps de décisions **échantillonné** (`id % modulo = 0` : le
    /// ranking du suggest ne consomme que des fréquences relatives). Renvoie
    /// `(id, full_text)` au-delà de `after_id`, ordre d'id.
    #[tracing::instrument(name = "db.suggest_decision_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_decision_texts_batch(
        &self,
        after_id: i64,
        modulo: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT t.decision_id, t.full_text FROM decision_text t
                     WHERE t.decision_id > $1 AND t.decision_id % $2 = 0
                       AND t.deleted_at IS NULL AND {read}
                     ORDER BY t.decision_id LIMIT $3",
                    read = super::pick::read("t")
                ),
                &[&after_id, &modulo, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Sampled keyset batch of article bodies (`id % modulo = 0`), in id order.
    #[tracing::instrument(name = "db.suggest_article_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_article_texts_batch(
        &self,
        after_id: i64,
        modulo: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, texte FROM legal_article
                 WHERE id > $1 AND id % $2 = 0 AND texte IS NOT NULL
                 ORDER BY id LIMIT $3",
                &[&after_id, &modulo, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Les titres de `legal_text` visibles en recherche — les nominations, véhicules
    /// de publication et lois d'habilitation ne sont pas suggérés. Injectés entiers
    /// dans le vocabulaire du mode textes avec boost de df.
    #[tracing::instrument(name = "db.suggest_text_titles", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_text_titles(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT title FROM legal_text
                 WHERE title IS NOT NULL
                   AND role NOT IN ('individuel', 'vehicule', 'habilitation')",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }
}
