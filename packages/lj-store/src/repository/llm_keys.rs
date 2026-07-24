//! Statut des clés Mistral du pool chat (table `mistral_key_status`) :
//! empreinte xxh3-64, jamais le secret.

use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// Empreintes des clés actuellement désactivées.
    #[tracing::instrument(
        name = "db.disabled_mistral_key_fingerprints",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn disabled_mistral_key_fingerprints(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT fingerprint FROM mistral_key_status WHERE disabled_until > now()",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Désactive une clé (empreinte) jusqu'au 2 du mois suivant — un jour de
    /// marge, l'heure exacte du reset mensuel n'étant pas documentée. Upsert
    /// idempotent.
    #[tracing::instrument(
        name = "db.mark_mistral_key_disabled",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn mark_mistral_key_disabled(
        &self,
        fingerprint: &str,
        last_status: i16,
        marked_by: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO mistral_key_status
                  (fingerprint, disabled_until, last_status, marked_by, updated_at)
                VALUES ($1, date_trunc('month', now()) + interval '1 month 1 day', $2, $3, now())
                ON CONFLICT (fingerprint) DO UPDATE SET
                  disabled_until = EXCLUDED.disabled_until,
                  last_status = EXCLUDED.last_status,
                  marked_by = EXCLUDED.marked_by,
                  updated_at = now()
                ",
                &[&fingerprint, &last_status, &marked_by],
            )
            .await?;
        Ok(())
    }
}
