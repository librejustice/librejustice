-- ADR 0127 — autorité « FR-prioritaire » : départage des provenances de même
-- `source_rank` par langue servie, AVANT l'`id` (ordre d'ingest). Sans cela, la
-- rendition EN d'une décision multilingue ingérée en premier (HUDOC publie un
-- `itemid` par langue → un `source_uid` par langue, même `source_rank`) reste
-- l'autorité quand le FR arrive ensuite : `ORDER BY source_rank DESC, id ASC`
-- est aveugle à la langue, l'EN garde le plus petit `id` et le `full_text` gravé
-- ne se re-dérive jamais (reconcile ne touche pas au texte). Conséquence : on
-- sert l'EN alors qu'on a le FR — l'inverse de l'intention FR-prioritaire d'ADR 0120.
--
-- `lang_rank` factorise la définition de « FR » en un seul endroit (CEDH :
-- `languageisocode = 'FRE'` ; CJUE : `resource_obtained_language = 'fra'`), à
-- intercaler dans le tri d'autorité : `ORDER BY source_rank DESC,
-- lang_rank(source_fields) DESC, id ASC`. `lang_rank = 0` pour toute provenance
-- non-FR (et pour `source_fields` NULL) ⇒ tri strictement inchangé partout
-- ailleurs (sources mono-langue, CJUE à provenance unique par CELEX).
CREATE FUNCTION lang_rank(fields jsonb) RETURNS int
    IMMUTABLE LANGUAGE sql AS $$
    SELECT CASE
        WHEN fields->>'languageisocode' = 'FRE'
          OR fields->>'resource_obtained_language' = 'fra'
        THEN 1 ELSE 0
    END
$$;
