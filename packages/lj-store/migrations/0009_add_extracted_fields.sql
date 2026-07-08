-- Extracted fields stored on decisions for filtering.
-- Codes are StrEnum values (e.g. SATISFACTION_TOTALE, REFERE_SUSPENSION).
-- is_recueil / is_tables_lebon are generated from publication_code for UI facets.
ALTER TABLE decisions
  ADD COLUMN IF NOT EXISTS date_lecture         DATE,
  ADD COLUMN IF NOT EXISTS date_audience        DATE,
  ADD COLUMN IF NOT EXISTS docket_numbers       TEXT[],
  ADD COLUMN IF NOT EXISTS jurisdiction_level   TEXT,
  ADD COLUMN IF NOT EXISTS instance_level       TEXT,
  ADD COLUMN IF NOT EXISTS formation_or_chamber TEXT,
  ADD COLUMN IF NOT EXISTS publication_code     TEXT,
  ADD COLUMN IF NOT EXISTS main_outcome         TEXT,
  ADD COLUMN IF NOT EXISTS special_procedure    TEXT,
  ADD COLUMN IF NOT EXISTS legal_references     JSONB;

ALTER TABLE decisions
  ADD COLUMN IF NOT EXISTS is_recueil     BOOLEAN
    GENERATED ALWAYS AS (publication_code LIKE '%A%') STORED,
  ADD COLUMN IF NOT EXISTS is_tables_lebon BOOLEAN
    GENERATED ALWAYS AS (publication_code LIKE '%B%') STORED;

CREATE INDEX IF NOT EXISTS idx_decisions_date_lecture
  ON decisions (date_lecture) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_jurisdiction_level
  ON decisions (jurisdiction_level) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_instance_level
  ON decisions (instance_level) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_main_outcome
  ON decisions (main_outcome) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_special_procedure
  ON decisions (special_procedure) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_is_recueil
  ON decisions (is_recueil) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_docket_numbers
  ON decisions USING GIN (docket_numbers) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_legal_references
  ON decisions USING GIN (legal_references) WHERE deleted_at IS NULL;
