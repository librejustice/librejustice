# ============================================================================
# CONTRACT TESTS — GET /api/search-textes  (recherche d'articles, ADR 0114)
# ----------------------------------------------------------------------------
# Asserts HTTP status + JSON STRUCTURAL invariants only.
# Variable: {{base}} = host root (e.g. http://localhost:8300).
# Route publique, sans auth. `q` vide ⇒ 4xx (même frontière que /api/search).
# ============================================================================

# ── 1. Baseline : recherche plein-texte d'articles ────────────────────────────
GET {{base}}/api/search-textes?q=clause%20abusive
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.total" isInteger
jsonpath "$.total" >= 0
jsonpath "$.hits" isCollection
# facets propres aux articles : code / jurisdiction / nature / source
jsonpath "$.facets.code" isCollection
jsonpath "$.facets.jurisdiction" isCollection
jsonpath "$.facets.nature" isCollection
jsonpath "$.facets.source" isCollection


# ── 2. Hit shape (assert sur le premier hit) ─────────────────────────────────
GET {{base}}/api/search-textes?q=d%C3%A9pens&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" count <= 5
jsonpath "$.hits[0].code" isString
jsonpath "$.hits[0].codeTitle" isString
jsonpath "$.hits[0].num" isString
jsonpath "$.hits[0].numKey" isString
jsonpath "$.hits[0].snippet" isString
jsonpath "$.hits[0].source" isString


# ── 3. q vide ⇒ 4xx (même frontière de validation que /api/search) ──────────
GET {{base}}/api/search-textes?q=
HTTP *
[Asserts]
status >= 400
status < 500


# ── 4. Filtre jurisdiction (FRANCE / UE / INTL) ──────────────────────────────
GET {{base}}/api/search-textes?q=contrat&jurisdiction=FRANCE
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 5. Filtre nature ─────────────────────────────────────────────────────────
GET {{base}}/api/search-textes?q=travail&nature=CODE
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 6. Filtre source ─────────────────────────────────────────────────────────
GET {{base}}/api/search-textes?q=contrat&source=legifrance
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 7. Scoped à un code (slug) ───────────────────────────────────────────────
GET {{base}}/api/search-textes?q=d%C3%A9pens&code=code-de-procedure-civile
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 8. limit hors bornes ⇒ 4xx ───────────────────────────────────────────────
GET {{base}}/api/search-textes?q=test&limit=101
HTTP *
[Asserts]
status >= 400
status < 500
