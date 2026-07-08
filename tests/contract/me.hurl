# ============================================================================
# Contract tests — route group `me` (profile, activity-tracking, bookmarks,
# search-history, decision-views). All routes require auth (RequiredUser).
#
# These tests assert the UNAUTHENTICATED contract (401 shape) for every
# endpoint, plus structural invariants on the authed responses. The runner
# injects {{token}} for the authed section.
#
# KNOWN PARITY RISK (see notes): the 401 BODY differs between oracles.
#   Python (FastAPI HTTPException) -> JSON  {"detail":"auth_required"}
#   Rust   (ApiError::Unauthorized) -> text/plain  "unauthorized"
# We assert ONLY status==401 here so the file is green against BOTH oracles;
# the body divergence is documented for the A/B differ to flag.
# ============================================================================

# ── GET /api/me — unauth ────────────────────────────────────────────────────
GET {{base}}/api/me
HTTP 401

# ── GET /api/me — bogus bearer (invalid token -> optional_user None -> 401) ──
GET {{base}}/api/me
Authorization: Bearer not-a-real-jwt
HTTP 401

# ── PATCH /api/me — unauth (auth checked before body validation) ────────────
PATCH {{base}}/api/me
Content-Type: application/json
{
  "displayName": "Jean Dupont"
}
HTTP 401

# ── DELETE /api/me — unauth ──────────────────────────────────────────────────
DELETE {{base}}/api/me
HTTP 401

# ── PUT /api/me/activity-tracking — unauth ──────────────────────────────────
PUT {{base}}/api/me/activity-tracking
Content-Type: application/json
{
  "enabled": false
}
HTTP 401

# ── GET /api/me/bookmarks — unauth ──────────────────────────────────────────
GET {{base}}/api/me/bookmarks
HTTP 401

# ── PUT /api/me/bookmarks/{id} — unauth ─────────────────────────────────────
PUT {{base}}/api/me/bookmarks/ABC123
HTTP 401

# ── DELETE /api/me/bookmarks/{id} — unauth ──────────────────────────────────
DELETE {{base}}/api/me/bookmarks/ABC123
HTTP 401

# ── GET /api/me/search-history — unauth ─────────────────────────────────────
GET {{base}}/api/me/search-history
HTTP 401

# ── GET /api/me/search-history with paging params — unauth ──────────────────
GET {{base}}/api/me/search-history?limit=10&offset=0
HTTP 401

# ── DELETE /api/me/search-history/{entry_id} — unauth ───────────────────────
DELETE {{base}}/api/me/search-history/1
HTTP 401

# ── DELETE /api/me/search-history (clear all) — unauth ──────────────────────
DELETE {{base}}/api/me/search-history
HTTP 401

# ── GET /api/me/decision-views — unauth ─────────────────────────────────────
GET {{base}}/api/me/decision-views
HTTP 401

# ── POST /api/me/decision-views/{id} — unauth ───────────────────────────────
POST {{base}}/api/me/decision-views/ABC123
HTTP 401

# ── DELETE /api/me/decision-views/{id} — unauth ─────────────────────────────
DELETE {{base}}/api/me/decision-views/ABC123
HTTP 401

# ── DELETE /api/me/decision-views (clear all) — unauth ──────────────────────
DELETE {{base}}/api/me/decision-views
HTTP 401

# ============================================================================
# AUTHED CONTRACT (runner injects {{token}}). These assert STRUCTURE only —
# field presence/types, camelCase keys, enum value sets, array ordering — not
# live data values. Uncomment in the authed runner.
# ============================================================================

# GET {{base}}/api/me
# Authorization: Bearer {{token}}
# HTTP 200
# [Asserts]
# header "Content-Type" contains "application/json"
# jsonpath "$.sub" isString
# jsonpath "$.email" exists                 # string or null
# jsonpath "$.displayName" exists           # camelCase; string or null
# jsonpath "$.createdAt" isString           # RFC3339 (micros, Z)
# jsonpath "$.lastSeenAt" isString
# jsonpath "$.trackActivity" isBoolean

# PATCH {{base}}/api/me
# Authorization: Bearer {{token}}
# Content-Type: application/json
# { "displayName": "Jean Dupont" }
# HTTP 200
# [Asserts]
# jsonpath "$.displayName" == "Jean Dupont"
# jsonpath "$.sub" isString
# jsonpath "$.trackActivity" isBoolean

# PUT {{base}}/api/me/activity-tracking
# Authorization: Bearer {{token}}
# Content-Type: application/json
# { "enabled": true }
# HTTP 200
# [Asserts]
# jsonpath "$.trackActivity" == true        # returns full UserProfile

# GET {{base}}/api/me/bookmarks
# Authorization: Bearer {{token}}
# HTTP 200
# [Asserts]
# jsonpath "$.items" isCollection
# jsonpath "$.total" isInteger
# jsonpath "$.total" >= 0
# # When items present (most-recent-first by bookmarked_at):
# # jsonpath "$.items[0].id" isString
# # jsonpath "$.items[0].title" isString
# # jsonpath "$.items[0].juridictionType" matches /^(TA|CAA|CE|CC|CA|TJ|TCOM)$/
# # jsonpath "$.items[0].bookmarkedAt" isString

# PUT {{base}}/api/me/bookmarks/{{decision_id}}
# Authorization: Bearer {{token}}
# HTTP 204               # idempotent (ON CONFLICT DO NOTHING)

# DELETE {{base}}/api/me/bookmarks/{{decision_id}}
# Authorization: Bearer {{token}}
# HTTP 204

# PUT {{base}}/api/me/bookmarks/__nonexistent__
# Authorization: Bearer {{token}}
# HTTP 404               # decision_not_found

# GET {{base}}/api/me/search-history?limit=10&offset=0
# Authorization: Bearer {{token}}
# HTTP 200
# [Asserts]
# jsonpath "$.items" isCollection
# jsonpath "$.total" isInteger
# # jsonpath "$.items[0].id" isInteger
# # jsonpath "$.items[0].query" isString
# # jsonpath "$.items[0].filters" exists           # object
# # jsonpath "$.items[0].source" matches /^(web|mcp)$/
# # jsonpath "$.items[0].createdAt" isString

# DELETE {{base}}/api/me/search-history/999999999
# Authorization: Bearer {{token}}
# HTTP 404               # entry_not_found (not owned / absent)

# DELETE {{base}}/api/me/search-history
# Authorization: Bearer {{token}}
# HTTP 204               # clear all

# GET {{base}}/api/me/decision-views?limit=10&offset=0
# Authorization: Bearer {{token}}
# HTTP 200
# [Asserts]
# jsonpath "$.items" isCollection
# jsonpath "$.total" isInteger
# # jsonpath "$.items[0].id" isString
# # jsonpath "$.items[0].viewCount" isInteger
# # jsonpath "$.items[0].lastSource" matches /^(web|mcp)$/
# # jsonpath "$.items[0].lastViewedAt" isString

# POST {{base}}/api/me/decision-views/{{decision_id}}
# Authorization: Bearer {{token}}
# HTTP 204

# POST {{base}}/api/me/decision-views/__nonexistent__
# Authorization: Bearer {{token}}
# HTTP 404               # decision_not_found

# DELETE {{base}}/api/me/decision-views/__nonexistent__
# Authorization: Bearer {{token}}
# HTTP 404               # decision_not_found (resolve fails before delete)

# DELETE {{base}}/api/me/decision-views
# Authorization: Bearer {{token}}
# HTTP 204               # clear all

