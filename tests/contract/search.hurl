# ============================================================================
# CONTRACT TESTS — GET /api/search
# ----------------------------------------------------------------------------
# Asserts HTTP status + JSON STRUCTURAL invariants only (field presence, types,
# enum value sets, ordering contracts). NOT exact live-data values.
# Variable: {{base}} = host root (e.g. http://localhost:8300).
# The route is auth-OPTIONAL: public callers get results; a JWT only triggers
# search-history recording (no response-shape change).
# Facettes = référentiel ADR 0146 (juridiction/legalDomain/solution/publication/
# dateLectureYear/legalInstrument).
# ============================================================================

# ── Health (sanity / smoke) ─────────────────────────────────────────────────
GET {{base}}/api/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString


# ── 1. Baseline hybrid search (mode=auto, natural language → hybrid) ─────────
GET {{base}}/api/search?q=expulsion%20locative
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.query" == "expulsion locative"
jsonpath "$.total" isInteger
jsonpath "$.total" >= 0
jsonpath "$.hits" isCollection
jsonpath "$.queryMode" isString
# queryMode enum set
jsonpath "$.queryMode" matches /^(lexical|hybrid)$/
# auto + natural language ⇒ resolved hybrid
jsonpath "$.queryMode" == "hybrid"
jsonpath "$.allHitIds" isCollection
jsonpath "$.facets" exists
# facets struct (ADR 0146): every dimension present as an array
jsonpath "$.facets.juridiction" isCollection
jsonpath "$.facets.legalDomain" isCollection
jsonpath "$.facets.solution" isCollection
jsonpath "$.facets.publication" isCollection
jsonpath "$.facets.dateLectureYear" isCollection
jsonpath "$.facets.legalInstrument" isCollection
# total never exceeds the absolute cap of 100 (MAX_RESULTS)
jsonpath "$.total" <= 100
# allHitIds length tracks total (full ranked list, not just the page)
jsonpath "$.allHitIds" count <= 100


# ── 2. Hit shape contract (only assert when at least one hit exists) ─────────
GET {{base}}/api/search?q=responsabilite&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" count <= 5
# When hits exist, each carries the required SearchHit fields.
jsonpath "$.hits[0].id" isString
jsonpath "$.hits[0].juridictionType" matches /^(TA|CAA|CE|CONSTIT|TC|CC|CA|TJ|TCOM|CEDH|CJUE|CNDA)$/
jsonpath "$.hits[0].titleHtml" isString
jsonpath "$.hits[0].score" isNumber
jsonpath "$.hits[0].bestChunk" exists
jsonpath "$.hits[0].bestChunk.chunkIndex" isInteger
jsonpath "$.hits[0].bestChunk.snippet" isString
jsonpath "$.hits[0].publicationCodes" isCollection


# ── 3. mode=lexical (forces boolean_search path) ─────────────────────────────
GET {{base}}/api/search?q=licenciement&mode=lexical
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.hits" isCollection


# ── 4. mode=semantic (forces hybrid; there is NO pure-ANN path) ──────────────
GET {{base}}/api/search?q=trouble%20du%20voisinage&mode=semantic
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "hybrid"


# ── 5. Boolean operator ET (auto-detected → lexical) ─────────────────────────
GET {{base}}/api/search?q=cong%C3%A9s%20ET%20pay%C3%A9s
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.hits" isCollection


# ── 6. Boolean operator OU ───────────────────────────────────────────────────
GET {{base}}/api/search?q=bail%20OU%20loyer
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 7. Boolean operator SAUF (negation) ──────────────────────────────────────
GET {{base}}/api/search?q=cong%C3%A9s%20SAUF%20maladie
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 8. Quoted phrase (triggers BOOL_RE via the quote char → lexical) ─────────
GET {{base}}/api/search?q=%22travail%20dominical%22
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 9. PROCHEn proximity operator → slop, lexical ────────────────────────────
GET {{base}}/api/search?q=%22travail%20dominical%22PROCHE3
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 10. Wildcard `*` → lexical ───────────────────────────────────────────────
GET {{base}}/api/search?q=expuls*
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 11. sort=date_desc ───────────────────────────────────────────────────────
GET {{base}}/api/search?q=urbanisme&sort=date_desc
HTTP 200
[Asserts]
jsonpath "$.queryMode" exists
jsonpath "$.hits" isCollection


# ── 12. sort=date_asc ────────────────────────────────────────────────────────
GET {{base}}/api/search?q=urbanisme&sort=date_asc
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 13. Pagination: offset beyond results returns empty page (total kept) ────
GET {{base}}/api/search?q=urbanisme&offset=99&limit=1
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.hits" count <= 1
jsonpath "$.total" isInteger
jsonpath "$.allHitIds" isCollection


# ── 14. Single filter: juridictionType (repeated query param) ────────────────
GET {{base}}/api/search?q=contrat&juridictionType=CE&juridictionType=CAA
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.facets.juridiction" isCollection


# ── 15. Single filter: solution (référentiel solution:*, ADR 0146) ───────────
GET {{base}}/api/search?q=rejet&solution=REJET
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 16. Single filter: voie (référentiel voie:*) ─────────────────────────────
GET {{base}}/api/search?q=suspension&voie=REFERE_SUSPENSION
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 17. Single filter: office (référentiel office:*) ─────────────────────────
GET {{base}}/api/search?q=saisie&office=JEX
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 18. Single filter: legalDomain ──────────────────────────────────────────
GET {{base}}/api/search?q=bail&legalDomain=CIVIL
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 19. Single filter: jurisdictionCode (codes référentiel jurisdiction) ─────
GET {{base}}/api/search?q=permis&jurisdictionCode=ce
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 20. Single filter: legalInstrument (token = ref_text_uid, ADR 0145 M4) ───
GET {{base}}/api/search?q=responsabilite&legalInstrument=LEGITEXT000006070721
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 21. Single filter: legalArticle (clé composite ref_text_uid|ref_num_key) ─
GET {{base}}/api/search?q=responsabilite&legalArticle=LEGITEXT000006070721%7C1240
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 22. Single filter: publication codes ─────────────────────────────────────
GET {{base}}/api/search?q=arret&publication=B
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 23. Date range filters ───────────────────────────────────────────────────
GET {{base}}/api/search?q=fiscalite&dateFrom=2020-01-01&dateTo=2023-12-31
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 24. Combined filters (many dimensions at once + date range) ──────────────
GET {{base}}/api/search?q=expulsion&juridictionType=CA&solution=CONFIRMATION&legalDomain=CIVIL_DROIT_LOCATIF&dateFrom=2018-01-01&dateTo=2024-12-31&publication=B
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.facets" exists


# ── 25. aiMode=true (rerank; summary embedded if present) ────────────────────
GET {{base}}/api/search?q=divorce&aiMode=true
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.queryMode" exists


# ── 26. legalInstrumentFacet ordering + nested articles contract ─────────────
GET {{base}}/api/search?q=responsabilite%20civile&limit=10
HTTP 200
[Asserts]
jsonpath "$.facets.legalInstrument" isCollection
# Each legalInstrument facet entry has value/label/count and an articles array
jsonpath "$.facets.legalInstrument[0].value" isString
jsonpath "$.facets.legalInstrument[0].label" isString
jsonpath "$.facets.legalInstrument[0].count" isInteger
jsonpath "$.facets.legalInstrument[0].articles" isCollection


# ── 27. juridiction facet = arbre 2 niveaux (racines uid, enfants parent) ────
GET {{base}}/api/search?q=permis%20de%20construire&limit=10
HTTP 200
[Asserts]
jsonpath "$.facets.juridiction" isCollection
jsonpath "$.facets.juridiction[0].value" isString
jsonpath "$.facets.juridiction[0].label" isString
jsonpath "$.facets.juridiction[0].count" isInteger
# racines = uids complets du référentiel (juridiction:TA / office:JEX)
jsonpath "$.facets.juridiction[0].value" matches /^(juridiction|office):/


# ============================================================================
# ERROR / VALIDATION CONTRACTS
# Rust (parse_search_query) returns HTTP 400/422 on invalid query params —
# assert the 4xx class, not the exact code.
# ============================================================================

# ── 28. Empty q (min_length=1 violated) ──────────────────────────────────────
GET {{base}}/api/search?q=
HTTP *
[Asserts]
status >= 400
status < 500


# ── 29. Missing q entirely ───────────────────────────────────────────────────
GET {{base}}/api/search
HTTP *
[Asserts]
status >= 400
status < 500


# ── 30. limit above max (le=50) ──────────────────────────────────────────────
GET {{base}}/api/search?q=test&limit=51
HTTP *
[Asserts]
status >= 400
status < 500


# ── 31. limit below min (ge=1) ───────────────────────────────────────────────
GET {{base}}/api/search?q=test&limit=0
HTTP *
[Asserts]
status >= 400
status < 500


# ── 32. offset above max (le=99) ─────────────────────────────────────────────
GET {{base}}/api/search?q=test&offset=100
HTTP *
[Asserts]
status >= 400
status < 500


# ── 33. Invalid mode enum ────────────────────────────────────────────────────
GET {{base}}/api/search?q=test&mode=fulltext
HTTP *
[Asserts]
status >= 400
status < 500


# ── 34. Invalid sort enum ────────────────────────────────────────────────────
GET {{base}}/api/search?q=test&sort=price
HTTP *
[Asserts]
status >= 400
status < 500


# ============================================================================
# AUTHED VARIANT (runner injects {{token}})
# The search endpoint is auth-OPTIONAL, so there is NO 401 contract: an
# unauthenticated call returns 200 with results. A bearer token only causes the
# query to be recorded in search-history; the response body is byte-identical.
# ============================================================================
