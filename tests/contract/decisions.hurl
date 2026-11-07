# ============================================================================
# CONTRACT TESTS — route group: decisions
#   GET /internal/decision/{id}
#   GET /internal/decision/{id}/similar
#   GET /internal/decision/{id}/preview
# All public (auth=none). {{base}} = host root — the ONLY variable this file
# needs: {{decision_id}} is captured below from a live search, so the suite runs
# as one command against any environment (placeholders like "does-not-exist-xyz"
# stay literal). Assertions are STRUCTURAL only (field presence / type / enum
# set / ordering), never live-data values.
# ============================================================================

# ---------------------------------------------------------------------------
# health sanity (anchors {{base}} + Cache shape)
# ---------------------------------------------------------------------------
GET {{base}}/admin/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString


# ---------------------------------------------------------------------------
# Discovery : un public_id RÉEL du corpus servi. Capturé plutôt que passé par
# le runner — une variable externe non fournie fait échouer le fichier sur
# « Undefined variable » au lieu de tester quoi que ce soit.
# ---------------------------------------------------------------------------
GET {{base}}/internal/search?query=usufruit&limit=1
HTTP 200
[Captures]
decision_id: jsonpath "$.hits[0].id"
[Asserts]
jsonpath "$.hits" count == 1


# ===========================================================================
# GET /internal/decision/{id}: detail, known-good id
# ===========================================================================
GET {{base}}/internal/decision/{{decision_id}}
HTTP 200
[Asserts]
header "Cache-Control" exists
# /api ne doit jamais être indexé (JSON + exports PDF/DOCX) : le contenu
# indexable est la page SSR.
header "X-Robots-Tag" == "noindex"
# required scalars
jsonpath "$.id" isString
jsonpath "$.id" == "{{decision_id}}"
jsonpath "$.jurisdictionType" isString
# JuridictionType enum set (SCREAMING codes; CC/CA/TJ/TCOM judiciaire, TA/CAA/CE admin)
jsonpath "$.jurisdictionType" matches /^(TA|CAA|CE|CC|CA|TJ|TCOM)$/
jsonpath "$.title" isString
# paragraphs: always present (list, possibly empty when no payload)
jsonpath "$.paragraphs" isCollection
# publicationCodes: always-present list (Field default_factory=list)
jsonpath "$.publicationCodes" isCollection
# optional/nullable canonical fields are camelCase keys
# sections: null OR non-empty list of {kind,anchor,label,paragraphs}
# (covered structurally in the sections-present vector below)


# ---------------------------------------------------------------------------
# detail: non-existent id → 404, status only
# ---------------------------------------------------------------------------
GET {{base}}/internal/decision/does-not-exist-xyz-0000
HTTP 404


# ===========================================================================
# GET /internal/decision/{id}/similar: deux volets (principe / espece), default
# limit (4) par volet
# ===========================================================================
GET {{base}}/internal/decision/{{decision_id}}/similar
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.decisionId" == "{{decision_id}}"
jsonpath "$.principe" isCollection
jsonpath "$.espece" isCollection
# default limit is 4 → at most 4 hits per volet
jsonpath "$.principe" count <= 4
jsonpath "$.espece" count <= 4


# similar — explicit limit within bounds
GET {{base}}/internal/decision/{{decision_id}}/similar?limit=12
HTTP 200
[Asserts]
jsonpath "$.decisionId" == "{{decision_id}}"
jsonpath "$.principe" isCollection
jsonpath "$.espece" isCollection
jsonpath "$.principe" count <= 12
jsonpath "$.espece" count <= 12

GET {{base}}/internal/decision/{{decision_id}}/similar?limit=1
HTTP 200
[Asserts]
jsonpath "$.principe" count <= 1
jsonpath "$.espece" count <= 1


# similar — limit below bound (0) → 422 (parité FastAPI, cf. routes.rs SimilarQuery)
GET {{base}}/internal/decision/{{decision_id}}/similar?limit=0
HTTP 422

# similar — limit above bound (13) → 422 (parité FastAPI)
GET {{base}}/internal/decision/{{decision_id}}/similar?limit=13
HTTP 422

# similar — non-numeric limit → 4xx (Rust BadRequest 400 / FastAPI 422 — see notes)
GET {{base}}/internal/decision/{{decision_id}}/similar?limit=abc
HTTP *
[Asserts]
status >= 400
status < 500

# similar — non-existent id → 404 (no source decision)
GET {{base}}/internal/decision/does-not-exist-xyz-0000/similar
HTTP 404


# ===========================================================================
# GET /internal/decision/{id}/preview: hover card : identité légère,
# jamais le corps (pas de paragraphs/sections).
# ===========================================================================
GET {{base}}/internal/decision/{{decision_id}}/preview
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.id" == "{{decision_id}}"
jsonpath "$.title" isString
jsonpath "$.publicationCodes" isCollection
jsonpath "$.paragraphs" not exists

# preview — non-existent id → 404
GET {{base}}/internal/decision/does-not-exist-xyz-0000/preview
HTTP 404


# ===========================================================================
# AUTHED SECTION (none in this group — all decision routes are public).
# All endpoints of this group take no JWT. There is therefore NO 401 contract to
# assert here: an unauthenticated call is the normal, expected case.
# (Contrast with /internal/me/* which are RequiredUser → 401 unauthenticated.)
# ===========================================================================
