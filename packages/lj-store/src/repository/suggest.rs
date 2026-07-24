//! Blob FST d'autocomplétion (`suggest_index`, ADR 0216) : écrit par
//! `lj-ingest build-suggest`, lu d'un bloc par `lj-server` au premier
//! `/suggest`.

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

    /// Lot keyset de corps de décisions **échantillonné** (`id % modulo = 0`,
    /// ADR 0216 : le ranking du suggest ne consomme que des fréquences
    /// relatives). Renvoie `(id, full_text)` au-delà de `after_id`, ordre d'id.
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
                "SELECT id, full_text FROM decisions
                 WHERE id > $1 AND id % $2 = 0
                   AND deleted_at IS NULL AND full_text IS NOT NULL
                 ORDER BY id LIMIT $3",
                &[&after_id, &modulo, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Lot keyset de corps d'articles (`legal_article.texte`, full scan — le
    /// corpus textes est petit). Renvoie `(id, texte)` au-delà de `after_id`.
    #[tracing::instrument(name = "db.suggest_article_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_article_texts_batch(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, texte FROM legal_article
                 WHERE id > $1 AND texte IS NOT NULL
                 ORDER BY id LIMIT $2",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Les titres de `legal_text` visibles en recherche (ADR 0246 §6 — les
    /// nominations, véhicules de publication et lois d'habilitation ne sont
    /// pas suggérés) — injectés entiers dans le vocabulaire du mode textes
    /// avec boost de df (ADR 0216).
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
