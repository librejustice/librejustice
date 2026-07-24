-- ADR 0253 : hubs juridiction (/juridictions, /juridiction/{code}/{annee}).
-- Les trois requêtes hub (catalogue agrégé, années d'un code, page d'une
-- année) filtrent et trient par (jurisdiction_code, date_lecture) ; seul
-- jurisdiction_type était indexé.
CREATE INDEX idx_decisions_jur_code_date
    ON decisions (jurisdiction_code, date_lecture DESC);
