-- ADR 0162 : slug universel des textes.
-- 1. Dédoublonnage des slugs existants : dans chaque groupe de doublons, le
--    plus petit text_uid garde le slug nu, les autres prennent le suffixe
--    `-{text_uid en minuscules}` (déterministe, jamais re-renommé).
UPDATE legal_text t
SET slug = t.slug || '-' || lower(t.text_uid)
WHERE t.slug IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM legal_text o
    WHERE o.slug = t.slug AND o.text_uid < t.text_uid
  );

-- 2. Unicité garantie par la base (resolve_referential_code fait un lookup
--    exact : deux textes sur un slug = un texte inaccessible).
DROP INDEX IF EXISTS idx_legal_text_slug;
CREATE UNIQUE INDEX idx_legal_text_slug ON legal_text (slug)
WHERE slug IS NOT NULL;
