-- Retrait d'alias_of (posé en 0157) : la fusion des manifestations d'un même
-- instrument passe par le collapse d'identité de l'ADR 0115 (ADR 0246 §3 amendé).
ALTER TABLE legal_text DROP COLUMN alias_of;
