-- ADR 0217 — renvois hyperliés dans les corps de normes : l'arête
-- norme→article au grain span, jumelle de legal_citation (0097) côté cible
-- et de text_case_citation (0136) côté émetteur. Émetteur = version
-- d'article (owner_text_uid, owner_num_key, owner_date_debut) ou, si les
-- deux derniers sont NULL, le corps legal_text.body. Offsets codepoints sur
-- le texte émetteur (convention 0143 : token identifiant). Seules les
-- citations RÉSOLUES sont stockées (ref_text_uid NOT NULL) : la cible est
-- le catalogue lui-même, un rejeu de la passe recalcule tout — pas de clé
-- pendante ni de relink. ref_num_key NULL = mention nue du texte.

CREATE TABLE text_legal_citation (
    owner_text_uid   text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    owner_num_key    text,          -- NULL = émis par le corps (legal_text.body)
    owner_date_debut date,          -- NULL = émis par le corps
    char_start       int4 NOT NULL,
    char_end         int4 NOT NULL,
    ref_text_uid     text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    ref_num_key      text,
    extract_version  int2 NOT NULL,
    CHECK (char_end > char_start),
    CHECK ((owner_num_key IS NULL) = (owner_date_debut IS NULL))
);
-- Identité d'un span (PK impossible : émetteur nullable) — rejouable sans doublon.
CREATE UNIQUE INDEX text_legal_citation_owner_span_key
    ON text_legal_citation (owner_text_uid, COALESCE(owner_num_key, ''),
                            COALESCE(owner_date_debut, '0001-01-01'::date), char_start);
