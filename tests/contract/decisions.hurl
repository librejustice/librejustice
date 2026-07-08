# ============================================================================
# CONTRACT TESTS — route group: decisions
#   GET /api/decision/{id}
#   GET /api/decision/{id}/similar
#   GET /api/decision/{id}/preview
# All public (auth=none). {{base}} = host root. {{decision_id}} = a REAL
# public_id discovered from a prior search (runner must substitute; placeholders
# like "does-not-exist-xyz" stay literal). Assertions are STRUCTURAL only
# (field presence / type / enum set / ordering), never live-data values.
# ============================================================================

# ---------------------------------------------------------------------------
# health sanity (anchors {{base}} + Cache shape)
# ---------------------------------------------------------------------------
GET {{base}}/api/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString


# ===========================================================================
# GET /api/decision/{id} — detail, known-good id
# ===========================================================================
GET {{base}}/api/decision/{{decision_id}}
HTTP 200
[Asserts]
header "Cache-Control" exists
# required scalars
jsonpath "$.id" isString
jsonpath "$.id" == "{{decision_id}}"
jsonpath "$.juridictionType" isString
# JuridictionType enum set (SCREAMING codes; CC/CA/TJ/TCOM judiciaire, TA/CAA/CE admin)
jsonpath "$.juridictionType" matches /^(TA|CAA|CE|CC|CA|TJ|TCOM)$/
jsonpath "$.title" isString
# paragraphs: always present (list, possibly empty when no payload)
jsonpath "$.paragraphs" isCollection
# publicationCodes: always-present list (Field default_factory=list)
jsonpath "$.publicationCodes" isCollection
# optional/nullable canonical fields are camelCase keys
# sections: null OR non-empty list of {kind,anchor,label,paragraphs}
# (covered structurally in the sections-present vector below)


# ---------------------------------------------------------------------------
# detail: non-existent id → 404 contract
#   Python (FastAPI): 404 JSON body {"detail":"not_found"}
#   Rust  (axum):     404 plain-text body "not found"
# We assert the status only (the body shape DIVERGES — see notes). The differ
# will flag the body; status parity is the contract that must hold.
# ---------------------------------------------------------------------------
GET {{base}}/api/decision/does-not-exist-xyz-0000
HTTP 404


# ===========================================================================
# GET /api/decision/{id}/similar — default limit (4)
# ===========================================================================
GET {{base}}/api/decision/{{decision_id}}/similar
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.decisionId" == "{{decision_id}}"
jsonpath "$.hits" isCollection
# default limit is 4 → at most 4 hits
jsonpath "$.hits" count <= 4


# similar — explicit limit within bounds
GET {{base}}/api/decision/{{decision_id}}/similar?limit=12
HTTP 200
[Asserts]
jsonpath "$.decisionId" == "{{decision_id}}"
jsonpath "$.hits" isCollection
jsonpath "$.hits" count <= 12

GET {{base}}/api/decision/{{decision_id}}/similar?limit=1
HTTP 200
[Asserts]
jsonpath "$.hits" count <= 1


# similar — limit below bound (0) → 422 (parité FastAPI, cf. routes.rs SimilarQuery)
GET {{base}}/api/decision/{{decision_id}}/similar?limit=0
HTTP 422

# similar — limit above bound (13) → 422 (parité FastAPI)
GET {{base}}/api/decision/{{decision_id}}/similar?limit=13
HTTP 422

# similar — non-numeric limit → 4xx (Rust BadRequest 400 / FastAPI 422 — see notes)
GET {{base}}/api/decision/{{decision_id}}/similar?limit=abc
HTTP *
[Asserts]
status >= 400
status < 500

# similar — non-existent id → 404 (no source decision)
GET {{base}}/api/decision/does-not-exist-xyz-0000/similar
HTTP 404


# ===========================================================================
# GET /api/decision/{id}/preview — hover card (ADR 0168) : identité légère,
# jamais le corps (pas de paragraphs/sections).
# ===========================================================================
GET {{base}}/api/decision/{{decision_id}}/preview
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.id" == "{{decision_id}}"
jsonpath "$.title" isString
jsonpath "$.publicationCodes" isCollection
jsonpath "$.paragraphs" not exists

# preview — non-existent id → 404
GET {{base}}/api/decision/does-not-exist-xyz-0000/preview
HTTP 404


# ===========================================================================
# AUTHED SECTION (none in this group — all decision routes are public).
# All endpoints of this group take no JWT. There is therefore NO 401 contract to
# assert here: an unauthenticated call is the normal, expected case.
# (Contrast with /api/me/* which are RequiredUser → 401 unauthenticated.)
# ===========================================================================
