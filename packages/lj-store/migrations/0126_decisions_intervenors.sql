-- Ontologie 0180 (rôle intervenant) : colonne NER intervenors sur decisions,
-- jumelle de la matrice applicant/defendant × counsel/firms/companies.
ALTER TABLE decisions ADD COLUMN intervenors text[] NOT NULL DEFAULT '{}';
